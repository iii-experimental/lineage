use crate::handlers::{
    StrategyContext, handle_disable, handle_enable, handle_hook, handle_resume, handle_rewind,
    handle_status,
};
use crate::{HookEvent, ResumeInput, RewindInput, StatusOutput};
use iii_sdk::{III, RegisterFunction, RegisterTriggerInput};
use serde_json::{Value, json};
use std::sync::Arc;

const HANDLE_ROUTE_FN_ID: &str = "lineage::handle_hook_route";

pub fn register(iii: &III) {
    let iii_arc = Arc::new(iii.clone());
    let ctx = Arc::new(StrategyContext::new(iii_arc.clone()));

    // Single shared HTTP function — engine routes all hook URLs to this id and
    // we dispatch on `path_params.agent` + `path_params.event`.
    {
        let ctx = ctx.clone();
        iii.register_function(RegisterFunction::new_async(
            HANDLE_ROUTE_FN_ID,
            move |req: Value| {
                let ctx = ctx.clone();
                async move { route_request(&ctx, req).await }
            },
        ));
    }

    register_http_trigger(iii, "POST", "hook/:agent/:event");

    register_status(iii, ctx.clone());
    register_rewind(iii, ctx.clone());
    register_resume(iii, ctx.clone());
    register_enable(iii, ctx.clone());
    register_disable(iii, ctx.clone());
}

fn register_http_trigger(iii: &III, method: &str, api_path: &str) {
    if let Err(e) = iii.register_trigger(RegisterTriggerInput {
        trigger_type: "http".to_string(),
        function_id: HANDLE_ROUTE_FN_ID.to_string(),
        config: json!({"api_path": api_path, "http_method": method}),
        metadata: None,
    }) {
        tracing::warn!(error = %e, "failed to register HTTP trigger");
    }
}

fn register_status(iii: &III, ctx: Arc<StrategyContext>) {
    iii.register_function(RegisterFunction::new_async(
        "lineage::status",
        move |_: Value| {
            let ctx = ctx.clone();
            async move { Ok::<StatusOutput, String>(handle_status(&ctx).await) }
        },
    ));
}

fn register_rewind(iii: &III, ctx: Arc<StrategyContext>) {
    iii.register_function(RegisterFunction::new_async(
        "lineage::rewind",
        move |input: RewindInput| {
            let ctx = ctx.clone();
            async move {
                handle_rewind(&ctx, input)
                    .await
                    .map(|_| json!({"ok": true}))
                    .map_err(|e| e.to_string())
            }
        },
    ));
}

fn register_resume(iii: &III, ctx: Arc<StrategyContext>) {
    iii.register_function(RegisterFunction::new_async(
        "lineage::resume",
        move |input: ResumeInput| {
            let ctx = ctx.clone();
            async move {
                handle_resume(&ctx, input)
                    .await
                    .map(|_| json!({"ok": true}))
                    .map_err(|e| e.to_string())
            }
        },
    ));
}

fn register_enable(iii: &III, ctx: Arc<StrategyContext>) {
    iii.register_function(RegisterFunction::new_async(
        "lineage::enable",
        move |_: Value| {
            let ctx = ctx.clone();
            async move {
                let enabled = handle_enable(&ctx).await;
                Ok::<Value, String>(json!({"enabled": enabled}))
            }
        },
    ));
}

fn register_disable(iii: &III, ctx: Arc<StrategyContext>) {
    iii.register_function(RegisterFunction::new_async(
        "lineage::disable",
        move |_: Value| {
            let ctx = ctx.clone();
            async move {
                let enabled = handle_disable(&ctx).await;
                Ok::<Value, String>(json!({"enabled": enabled}))
            }
        },
    ));
}

/// Handle one HTTP-routed hook invocation.
///
/// Returns `Ok` only when the hook lands cleanly. Any failure bubbles as
/// `Err(String)` so the iii engine renders the 500 itself, with OTel error
/// spans, dead-letter routing, and console traces all firing correctly.
/// We do NOT hand-craft a `{status_code: 500, ...}` envelope — that path
/// hides errors from the engine's own observability.
async fn route_request(ctx: &StrategyContext, req: Value) -> Result<Value, String> {
    let path_params = req.get("path_params").cloned().unwrap_or(Value::Null);
    let agent = path_params
        .get("agent")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "missing :agent in path".to_string())?;
    let event_slug = path_params
        .get("event")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "missing :event in path".to_string())?;
    let event = HookEvent::from_url_slug(&event_slug)
        .ok_or_else(|| format!("unknown event slug: {event_slug}"))?;
    let body = req.get("body").cloned().unwrap_or(Value::Null);

    let result = handle_hook(ctx, &agent, event, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "status_code": 200,
        "headers": {},
        "body": serde_json::to_value(result).map_err(|e| e.to_string())?,
    }))
}
