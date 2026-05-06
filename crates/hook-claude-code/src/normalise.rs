use serde_json::{Value, json};

/// Translate a Claude Code hook payload into the lineage `NormalisedEvent`
/// shape consumed by `lineage-strategy`.
///
/// Claude Code emits stdin JSON with these top-level fields:
/// - `session_id`, `transcript_path`, `cwd`
/// - `hook_event_name`: SessionStart | UserPromptSubmit | Stop | PreToolUse | PostToolUse
/// - `prompt` (UserPromptSubmit), `stop_hook_active`
/// - `tool_use_id`, `tool_name`, `tool_input`, `tool_response` (PreToolUse / PostToolUse)
pub fn normalise(raw: Value) -> anyhow::Result<Value> {
    let session_id = raw
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let transcript_path = raw
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let prompt = raw
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_use_id = raw
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_name = raw
        .get("tool_name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_input = raw.get("tool_input").cloned();
    let tool_result = raw.get("tool_response").cloned();

    let event = match raw
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
    {
        "SessionStart" => "session_start",
        "UserPromptSubmit" => "user_prompt_submit",
        "Stop" => "stop",
        "PreToolUse" => "pre_task",
        "PostToolUse" => {
            // Subagent task vs todo write
            match tool_name.as_deref() {
                Some("TodoWrite") => "todo",
                _ => "post_task",
            }
        }
        other => other,
    };

    Ok(json!({
        "agent": "claude-code",
        "event": event,
        "session_id": session_id,
        "parent_entry_id": Value::Null,
        "prompt": prompt,
        "transcript_path": transcript_path,
        "tool_use_id": tool_use_id,
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_result": tool_result,
        "raw": raw,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_submit_extracts_prompt() {
        let raw = json!({
            "session_id": "abc",
            "transcript_path": "/tmp/t.jsonl",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "ship it"
        });
        let n = normalise(raw).unwrap();
        assert_eq!(n["event"], "user_prompt_submit");
        assert_eq!(n["session_id"], "abc");
        assert_eq!(n["prompt"], "ship it");
        assert_eq!(n["agent"], "claude-code");
    }

    #[test]
    fn post_tool_use_with_todowrite_maps_to_todo() {
        let raw = json!({
            "session_id": "abc",
            "hook_event_name": "PostToolUse",
            "tool_name": "TodoWrite",
            "tool_use_id": "tu1",
            "tool_input": {"todos": []}
        });
        let n = normalise(raw).unwrap();
        assert_eq!(n["event"], "todo");
        assert_eq!(n["tool_name"], "TodoWrite");
    }

    #[test]
    fn post_tool_use_with_other_tool_maps_to_post_task() {
        let raw = json!({
            "session_id": "abc",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_use_id": "tu1"
        });
        let n = normalise(raw).unwrap();
        assert_eq!(n["event"], "post_task");
    }

    #[test]
    fn pre_tool_use_maps_to_pre_task() {
        let raw = json!({
            "session_id": "abc",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
        });
        let n = normalise(raw).unwrap();
        assert_eq!(n["event"], "pre_task");
    }
}
