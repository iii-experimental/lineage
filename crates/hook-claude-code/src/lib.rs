use serde::{Deserialize, Serialize};

/// Raw Claude Code hook payload, read from stdin by the `claude` runtime
/// then forwarded to `lineage-strategy` via HTTP.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RawClaudeCode {
    pub session_id: Option<String>,
    pub session_ref: Option<String>,
    pub transcript_path: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_response: Option<serde_json::Value>,
    pub user_prompt: Option<String>,
}

pub mod normalise;
pub mod registration;
