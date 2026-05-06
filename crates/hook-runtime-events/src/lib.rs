//! Generic hook normaliser.
//!
//! The runtime hook payloads shipped by Codex, Gemini CLI, OpenCode, Cursor,
//! GitHub Copilot CLI, and Factory.ai Droid all share a common shape:
//!
//! `{ event, session_id, prompt?, call_id|tool_use_id?, tool|tool_name?, input|tool_input?, output|tool_result?, transcript_path? }`
//!
//! This crate registers `hook::runtime_events::*` functions that match that
//! shape. Claude Code uses a different vocabulary (`hook_event_name`,
//! `tool_response`); see `hook-claude-code` for that.
//!
//! Why one crate instead of six: every variant differs only by the literal
//! agent name. The right axis of decomposition is per-payload-shape, not
//! per-agent-name. Adding a new runtime that conforms to the shape requires
//! zero new code — just route to this normaliser.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RawRuntimeEvent {
    #[serde(default)]
    pub event: Option<String>,
    pub session_id: Option<String>,
    pub session_ref: Option<String>,
    pub transcript_path: Option<String>,
    pub call_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool: Option<String>,
    pub tool_name: Option<String>,
    pub input: Option<serde_json::Value>,
    pub tool_input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub tool_result: Option<serde_json::Value>,
    pub prompt: Option<String>,
}

pub mod normalise;
pub mod registration;
