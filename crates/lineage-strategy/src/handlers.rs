use crate::{HookEvent, HookResult, NormalisedEvent, ResumeInput, RewindInput, StatusOutput};
use iii_sdk::{III, TriggerAction, TriggerRequest};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Strategy worker state.
///
/// Uses `parking_lot::Mutex` (no poisoning semantics) instead of `std::sync::Mutex`
/// so a panic in one hook handler doesn't permanently disable subsequent hooks.
pub struct StrategyContext {
    pub iii: Arc<III>,
    pub repo_path: Mutex<String>,
    pub current_session: Mutex<Option<String>>,
    /// Sessions we have already issued `session::create` for. Skips the redundant
    /// extra round-trip on every subsequent hook for a known session.
    pub created_sessions: Mutex<HashSet<String>>,
    /// Capture kill-switch. Defaults to true. `lineage disable` flips off,
    /// `lineage enable` flips back on. In-memory only for v0.2; resets on
    /// engine restart (a v0.3 follow-up will persist this in iii-state).
    pub enabled: AtomicBool,
}

impl StrategyContext {
    pub fn new(iii: Arc<III>) -> Self {
        Self {
            iii,
            repo_path: Mutex::new(
                std::env::var("LINEAGE_REPO_PATH").unwrap_or_else(|_| ".".into()),
            ),
            current_session: Mutex::new(None),
            created_sessions: Mutex::new(HashSet::new()),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn repo_path(&self) -> String {
        self.repo_path.lock().clone()
    }

    pub fn set_session(&self, sid: Option<String>) {
        *self.current_session.lock() = sid;
    }

    pub fn current_session(&self) -> Option<String> {
        self.current_session.lock().clone()
    }

    /// Returns true if this is the first time we're seeing this session.
    /// Caller should issue `session::create` only on first-seen.
    pub fn mark_session_seen(&self, sid: &str) -> bool {
        self.created_sessions.lock().insert(sid.to_string())
    }
}

async fn trigger(iii: &III, function_id: &str, payload: Value) -> anyhow::Result<Value> {
    let res = iii
        .trigger(TriggerRequest {
            function_id: function_id.to_string(),
            payload,
            action: None,
            timeout_ms: Some(5_000),
        })
        .await?;
    Ok(res)
}

/// Detect which payload shape the raw event matches by fanning out to every
/// registered `hook::*::detect` function and picking the first match.
///
/// Falls back to a hardcoded URL-agent dispatch if hook-fanout isn't on the
/// bus or returns no match. The URL-path agent is also used as the agent
/// label inside the normalised event regardless of detected shape.
async fn detect_shape(iii: &III, raw: &Value) -> Option<&'static str> {
    let res = trigger(
        iii,
        "hooks::publish_collect",
        json!({
            "topic": "lineage::detect",
            "payload": {"raw": raw},
            "merge_rule": "first_block_wins",
            "timeout_ms": 500,
        }),
    )
    .await
    .ok()?;
    let shape = res.get("shape").and_then(|v| v.as_str())?;
    match shape {
        "claude_code" => Some("claude_code"),
        "runtime_events" => Some("runtime_events"),
        _ => None,
    }
}

/// Direct (non-fanout) shape detection by inspecting the payload itself.
/// Used when hook-fanout isn't reachable.
fn shape_from_payload(raw: &Value) -> &'static str {
    if raw.get("hook_event_name").is_some() {
        "claude_code"
    } else {
        "runtime_events"
    }
}

/// Normalise a raw runtime payload by routing to the appropriate
/// per-shape `hook::<shape>::normalise` function.
async fn normalise(iii: &III, agent: &str, raw: Value) -> anyhow::Result<NormalisedEvent> {
    let shape = detect_shape(iii, &raw).await.unwrap_or_else(|| {
        tracing::debug!("hook-fanout unavailable, detecting shape from payload");
        shape_from_payload(&raw)
    });

    let res = match shape {
        "claude_code" => trigger(iii, "hook::claude_code::normalise", raw).await?,
        "runtime_events" => {
            trigger(
                iii,
                "hook::runtime_events::normalise",
                json!({"agent": agent, "raw": raw}),
            )
            .await?
        }
        other => return Err(anyhow::anyhow!("unknown payload shape: {other}")),
    };
    let evt: NormalisedEvent = serde_json::from_value(res)?;
    Ok(evt)
}

/// Append a session-tree entry. Creates the session on first-seen only.
async fn append_entry(
    iii: &III,
    evt: &NormalisedEvent,
    is_first_seen: bool,
) -> anyhow::Result<String> {
    if is_first_seen {
        if let Err(e) = trigger(
            iii,
            "session::create",
            json!({"session_id": evt.session_id}),
        )
        .await
        {
            // Don't fail the hook just because session-tree is unreachable.
            // The append below will still attempt; entry_id may come back empty.
            tracing::warn!(error = %e, session_id = %evt.session_id, "session::create failed; session-tree may not be registered");
        }
    }

    let entry = json!({
        "session_id": evt.session_id,
        "parent_id": evt.parent_entry_id,
        "kind": format!("{:?}", evt.event),
        "agent": evt.agent,
        "prompt": evt.prompt,
        "tool_use_id": evt.tool_use_id,
        "tool_name": evt.tool_name,
        "tool_input": evt.tool_input,
        "tool_result": evt.tool_result,
    });
    let res = trigger(iii, "session::append", entry).await?;
    let entry_id = res
        .get("entry_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    Ok(entry_id)
}

/// Enqueue a shadow snapshot through the `lineage_checkpoint` FIFO queue.
///
/// The queue's `message_group_field=session_id` guarantees that two snapshots
/// for the same session are processed serially, eliminating the race where two
/// parallel `Stop` hooks for the same session would both compute their tree
/// from the same working-dir state and clobber each other.
///
/// Returns an EnqueueResult.message_receipt_id; the actual `shadow_ref` is
/// produced asynchronously by the queue consumer (lineage-gitops) and emitted
/// via the checkpoint stream. The strategy returns the receipt id immediately
/// so hook latency stays low.
async fn enqueue_snapshot(
    iii: &III,
    repo_path: &str,
    session_id: &str,
    parent_entry_id: Option<String>,
) -> anyhow::Result<String> {
    let res = iii
        .trigger(TriggerRequest {
            function_id: "lineage::shadow_snapshot".to_string(),
            payload: json!({
                "session_id": session_id,
                "parent_entry_id": parent_entry_id,
                "repo_path": repo_path,
            }),
            action: Some(TriggerAction::Enqueue {
                queue: "lineage_checkpoint".to_string(),
            }),
            timeout_ms: Some(500),
        })
        .await?;
    Ok(res
        .get("messageReceiptId")
        .or_else(|| res.get("message_receipt_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default())
}

async fn publish_topic(iii: &III, topic: &str, payload: Value) {
    let _ = trigger(
        iii,
        "iii::durable::publish",
        json!({"topic": topic, "data": payload}),
    )
    .await;
}

/// Main entry point — runs all six lifecycle events through one path.
pub async fn handle_hook(
    ctx: &StrategyContext,
    agent: &str,
    event: HookEvent,
    raw: Value,
) -> anyhow::Result<HookResult> {
    if !ctx.is_enabled() {
        // Kill-switch: acknowledge the hook so the runtime stays responsive,
        // but write nothing. session_id is best-effort — pulled from the raw
        // payload if present, otherwise empty.
        let session_id = raw
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        return Ok(HookResult {
            session_id,
            entry_id: String::new(),
            shadow_ref: None,
            blocked: true,
            message: Some("lineage capture is disabled. Run `lineage enable` to resume.".into()),
        });
    }
    let mut evt = normalise(&ctx.iii, agent, raw).await?;
    evt.event = event;
    evt.agent = agent.to_string();
    if evt.parent_entry_id.is_none() {
        // best-effort: ask session-tree for the active path's tail
        if let Ok(res) = trigger(
            &ctx.iii,
            "session::active_path",
            json!({"session_id": evt.session_id}),
        )
        .await
            && let Some(tail) = res
                .get("entries")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|e| e.get("entry_id"))
                .and_then(|v| v.as_str())
        {
            evt.parent_entry_id = Some(tail.to_string());
        }
    }

    let is_first_seen = ctx.mark_session_seen(&evt.session_id);
    let entry_id = append_entry(&ctx.iii, &evt, is_first_seen)
        .await
        .unwrap_or_default();

    let shadow_ref = if event.snapshot_after() {
        // Multi-tenant: prefer the cwd carried in the hook payload (one engine
        // can capture sessions for many repos concurrently). Fall back to the
        // worker-startup `LINEAGE_REPO_PATH` only when payload has no cwd.
        let p = evt.cwd.clone().unwrap_or_else(|| ctx.repo_path());
        match enqueue_snapshot(&ctx.iii, &p, &evt.session_id, Some(entry_id.clone())).await {
            Ok(receipt_id) => Some(format!("queued:{receipt_id}")),
            Err(e) => {
                tracing::warn!(error = %e, "enqueue snapshot failed");
                None
            }
        }
    } else {
        None
    };

    let topic = match event {
        HookEvent::SessionStart | HookEvent::UserPromptSubmit => "agent::events",
        HookEvent::PreTask => "agent::before_tool_call",
        HookEvent::PostTask | HookEvent::Stop | HookEvent::Todo => "agent::after_tool_call",
    };
    publish_topic(
        &ctx.iii,
        topic,
        serde_json::to_value(&evt).unwrap_or(Value::Null),
    )
    .await;

    if event == HookEvent::SessionStart {
        ctx.set_session(Some(evt.session_id.clone()));
    }

    Ok(HookResult {
        session_id: evt.session_id,
        entry_id,
        shadow_ref,
        blocked: false,
        message: None,
    })
}

pub async fn handle_status(ctx: &StrategyContext) -> StatusOutput {
    StatusOutput {
        enabled: ctx.is_enabled(),
        current_session_id: ctx.current_session(),
        repo_path: Some(ctx.repo_path()),
        engine_url: std::env::var("III_URL")
            .ok()
            .or_else(|| Some("ws://127.0.0.1:49234".to_string())),
        http_base: Some("http://127.0.0.1:3211".to_string()),
        console_hint: "open http://127.0.0.1:3213 for live worker / function / queue state"
            .to_string(),
    }
}

pub async fn handle_enable(ctx: &StrategyContext) -> bool {
    ctx.set_enabled(true);
    true
}

pub async fn handle_disable(ctx: &StrategyContext) -> bool {
    ctx.set_enabled(false);
    false
}

pub async fn handle_rewind(ctx: &StrategyContext, input: RewindInput) -> anyhow::Result<()> {
    let shadow_ref = format!(
        "{}/{}/{}",
        lineage_gitops::SHADOW_REF_PREFIX,
        input.session_id,
        input.entry_id
    );
    trigger(
        &ctx.iii,
        "lineage::rewind_to",
        json!({
            "repo_path": ctx.repo_path(),
            "shadow_ref": shadow_ref,
            "force": input.force,
        }),
    )
    .await?;
    Ok(())
}

pub async fn handle_resume(ctx: &StrategyContext, input: ResumeInput) -> anyhow::Result<()> {
    ctx.set_session(Some(input.session_id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_slug_parses_all_events() {
        assert_eq!(
            HookEvent::from_url_slug("user-prompt-submit"),
            Some(HookEvent::UserPromptSubmit)
        );
        assert_eq!(HookEvent::from_url_slug("stop"), Some(HookEvent::Stop));
        assert_eq!(
            HookEvent::from_url_slug("pre-task"),
            Some(HookEvent::PreTask)
        );
        assert_eq!(
            HookEvent::from_url_slug("post-task"),
            Some(HookEvent::PostTask)
        );
        assert_eq!(HookEvent::from_url_slug("todo"), Some(HookEvent::Todo));
        assert_eq!(
            HookEvent::from_url_slug("session-start"),
            Some(HookEvent::SessionStart)
        );
        assert_eq!(HookEvent::from_url_slug("nope"), None);
    }

    #[test]
    fn snapshot_after_only_for_completion_events() {
        assert!(HookEvent::Stop.snapshot_after());
        assert!(HookEvent::PostTask.snapshot_after());
        assert!(HookEvent::Todo.snapshot_after());
        assert!(!HookEvent::UserPromptSubmit.snapshot_after());
        assert!(!HookEvent::PreTask.snapshot_after());
        assert!(!HookEvent::SessionStart.snapshot_after());
    }

    #[test]
    fn shape_from_payload_picks_claude_code_when_hook_event_name_present() {
        assert_eq!(
            shape_from_payload(&json!({"hook_event_name": "Stop"})),
            "claude_code"
        );
        assert_eq!(
            shape_from_payload(&json!({"hook_event_name": "UserPromptSubmit", "session_id": "s"})),
            "claude_code"
        );
    }

    #[test]
    fn shape_from_payload_falls_back_to_runtime_events() {
        assert_eq!(
            shape_from_payload(&json!({"event": "stop", "session_id": "s"})),
            "runtime_events"
        );
        assert_eq!(shape_from_payload(&json!({})), "runtime_events");
    }

    #[test]
    fn mark_session_seen_returns_true_only_first_time() {
        // Cannot construct a real III without a connection; use a no-op stub.
        let iii = Arc::new(iii_sdk::III::new("ws://0.0.0.0:1"));
        let ctx = StrategyContext::new(iii);
        assert!(ctx.mark_session_seen("alpha"), "first sighting must be new");
        assert!(!ctx.mark_session_seen("alpha"), "second sighting is cached");
        assert!(ctx.mark_session_seen("beta"), "different session is new");
    }

    #[test]
    fn topic_routing_maps_each_hook_event_correctly() {
        // Replicates the match in handle_hook so any future change here triggers
        // a test failure rather than a silent runtime regression.
        fn topic_for(event: HookEvent) -> &'static str {
            match event {
                HookEvent::SessionStart | HookEvent::UserPromptSubmit => "agent::events",
                HookEvent::PreTask => "agent::before_tool_call",
                HookEvent::PostTask | HookEvent::Stop | HookEvent::Todo => "agent::after_tool_call",
            }
        }
        assert_eq!(topic_for(HookEvent::SessionStart), "agent::events");
        assert_eq!(topic_for(HookEvent::UserPromptSubmit), "agent::events");
        assert_eq!(topic_for(HookEvent::PreTask), "agent::before_tool_call");
        assert_eq!(topic_for(HookEvent::PostTask), "agent::after_tool_call");
        assert_eq!(topic_for(HookEvent::Stop), "agent::after_tool_call");
        assert_eq!(topic_for(HookEvent::Todo), "agent::after_tool_call");
    }

    #[test]
    fn strategy_context_repo_path_default() {
        // Unset the env so the default path applies.
        unsafe {
            std::env::remove_var("LINEAGE_REPO_PATH");
        }
        let iii = Arc::new(iii_sdk::III::new("ws://0.0.0.0:1"));
        let ctx = StrategyContext::new(iii);
        assert_eq!(ctx.repo_path(), ".");
    }

    #[test]
    fn strategy_context_session_set_get() {
        let iii = Arc::new(iii_sdk::III::new("ws://0.0.0.0:1"));
        let ctx = StrategyContext::new(iii);
        assert_eq!(ctx.current_session(), None);
        ctx.set_session(Some("s1".into()));
        assert_eq!(ctx.current_session(), Some("s1".into()));
        ctx.set_session(None);
        assert_eq!(ctx.current_session(), None);
    }
}
