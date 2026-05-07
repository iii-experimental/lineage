use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    UserPromptSubmit,
    Stop,
    PreTask,
    PostTask,
    Todo,
    SessionStart,
}

impl HookEvent {
    pub fn from_url_slug(s: &str) -> Option<Self> {
        Some(match s {
            "user-prompt-submit" | "user_prompt_submit" => Self::UserPromptSubmit,
            "stop" => Self::Stop,
            "pre-task" | "pre_task" => Self::PreTask,
            "post-task" | "post_task" => Self::PostTask,
            "todo" => Self::Todo,
            "session-start" | "session_start" => Self::SessionStart,
            _ => return None,
        })
    }

    pub fn snapshot_after(self) -> bool {
        matches!(self, Self::Stop | Self::PostTask | Self::Todo)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NormalisedEvent {
    pub agent: String,
    pub event: HookEvent,
    pub session_id: String,
    pub parent_entry_id: Option<String>,
    pub prompt: Option<String>,
    pub transcript_path: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_result: Option<serde_json::Value>,
    /// Working directory the runtime was launched from. Lifted from the
    /// hook payload's `cwd` (Claude Code's `cwd` field, generic runtimes'
    /// `cwd` or `repo_path`). Lets one engine capture sessions for
    /// multiple repos concurrently — strategy uses this for the snapshot
    /// path, falling back to `LINEAGE_REPO_PATH` only when payload has no
    /// `cwd`.
    pub cwd: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HookResult {
    pub session_id: String,
    pub entry_id: String,
    pub shadow_ref: Option<String>,
    pub blocked: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct StatusOutput {
    pub enabled: bool,
    pub current_session_id: Option<String>,
    pub repo_path: Option<String>,
    pub engine_url: Option<String>,
    pub http_base: Option<String>,
    /// Hint where to inspect live worker state. Lineage doesn't track
    /// connected workers itself; the iii-console at this URL renders
    /// the registered functions, queues, traces, and stream activity.
    pub console_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EnableInput {
    pub repo_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RewindInput {
    pub entry_id: String,
    pub session_id: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResumeInput {
    pub session_id: String,
}

pub mod handlers;
pub mod registration;
