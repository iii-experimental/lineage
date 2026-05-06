use serde_json::{Value, json};

/// Translate a generic runtime hook payload into the lineage NormalisedEvent
/// shape. Caller passes the agent name (e.g. "codex", "gemini-cli") since the
/// payload itself doesn't carry a runtime label.
pub fn normalise(agent: &str, raw: Value) -> anyhow::Result<Value> {
    let session_id = raw
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let prompt = raw
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_use_id = raw
        .get("call_id")
        .or_else(|| raw.get("tool_use_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_name = raw
        .get("tool")
        .or_else(|| raw.get("tool_name"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_input = raw.get("input").or_else(|| raw.get("tool_input")).cloned();
    let tool_result = raw
        .get("output")
        .or_else(|| raw.get("tool_result"))
        .cloned();

    let event = match raw.get("event").and_then(|v| v.as_str()).unwrap_or("") {
        "session_start" | "session-start" => "session_start",
        "prompt_submit" | "user_prompt_submit" | "user-prompt-submit" => "user_prompt_submit",
        "stop" | "turn_complete" | "turn-complete" => "stop",
        "tool_pre" | "pre_task" | "pre-task" => "pre_task",
        "tool_post" | "post_task" | "post-task" => "post_task",
        "todo" => "todo",
        other => other,
    };

    Ok(json!({
        "agent": agent,
        "event": event,
        "session_id": session_id,
        "parent_entry_id": Value::Null,
        "prompt": prompt,
        "transcript_path": raw.get("transcript_path"),
        "tool_use_id": tool_use_id,
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_result": tool_result,
        "raw": raw,
    }))
}

/// Detect whether a payload matches the generic runtime-events shape.
///
/// Returns `true` if the payload looks like a generic runtime event
/// (has an `event` field but NOT `hook_event_name`). Claude Code uses
/// `hook_event_name`, so this returns `false` for Claude Code payloads.
pub fn matches_shape(raw: &Value) -> bool {
    let has_event = raw.get("event").and_then(|v| v.as_str()).is_some();
    let has_claude_event_name = raw.get("hook_event_name").is_some();
    has_event && !has_claude_event_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_event_normalises_for_codex() {
        let raw = json!({"session_id": "s", "event": "stop"});
        let n = normalise("codex", raw).unwrap();
        assert_eq!(n["event"], "stop");
        assert_eq!(n["agent"], "codex");
    }

    #[test]
    fn prompt_submit_aliases() {
        for alias in ["prompt_submit", "user_prompt_submit", "user-prompt-submit"] {
            let raw = json!({"session_id": "s", "event": alias, "prompt": "hi"});
            let n = normalise("gemini-cli", raw).unwrap();
            assert_eq!(n["event"], "user_prompt_submit", "alias {alias} should map");
            assert_eq!(n["prompt"], "hi");
        }
    }

    #[test]
    fn pre_post_task_aliases() {
        for (alias, expected) in [
            ("pre_task", "pre_task"),
            ("pre-task", "pre_task"),
            ("tool_pre", "pre_task"),
            ("post_task", "post_task"),
            ("post-task", "post_task"),
            ("tool_post", "post_task"),
        ] {
            let raw = json!({"session_id": "s", "event": alias, "tool": "Bash"});
            let n = normalise("opencode", raw).unwrap();
            assert_eq!(
                n["event"], expected,
                "alias {alias} should map to {expected}"
            );
        }
    }

    #[test]
    fn input_and_output_both_lift_correctly() {
        let raw = json!({
            "session_id": "s",
            "event": "tool_post",
            "call_id": "c1",
            "tool": "FileWrite",
            "input": {"path": "/tmp/a"},
            "output": {"ok": true}
        });
        let n = normalise("cursor", raw).unwrap();
        assert_eq!(n["tool_use_id"], "c1");
        assert_eq!(n["tool_name"], "FileWrite");
        assert_eq!(n["tool_input"]["path"], "/tmp/a");
        assert_eq!(n["tool_result"]["ok"], true);
    }

    #[test]
    fn matches_shape_accepts_runtime_event_rejects_claude_code() {
        assert!(matches_shape(&json!({"event": "stop"})));
        assert!(!matches_shape(&json!({"hook_event_name": "Stop"})));
        assert!(!matches_shape(&json!({})));
    }

    #[test]
    fn missing_session_id_falls_back_to_unknown() {
        let raw = json!({"event": "stop"});
        let n = normalise("droid", raw).unwrap();
        assert_eq!(n["session_id"], "unknown");
    }

    #[test]
    fn unknown_event_passes_through_verbatim() {
        let raw = json!({"session_id": "s", "event": "custom_event"});
        let n = normalise("copilot-cli", raw).unwrap();
        assert_eq!(n["event"], "custom_event");
    }
}
