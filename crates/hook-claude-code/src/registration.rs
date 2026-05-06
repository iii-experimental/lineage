use crate::normalise;
use iii_sdk::{III, RegisterFunction};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DetectInput {
    pub raw: Value,
}

pub fn register(iii: &III) {
    iii.register_function(RegisterFunction::new(
        "hook::claude_code::normalise",
        |raw: Value| -> Result<Value, String> {
            normalise::normalise(raw).map_err(|e| e.to_string())
        },
    ));

    iii.register_function(RegisterFunction::new(
        "hook::claude_code::detect",
        |input: DetectInput| -> Result<Value, String> {
            // Payload-shape detection: Claude Code uses `hook_event_name`
            // (UserPromptSubmit, Stop, PreToolUse, PostToolUse, SessionStart).
            let m = input.raw.get("hook_event_name").is_some();
            Ok(json!({"match": m, "shape": "claude_code"}))
        },
    ));
}
