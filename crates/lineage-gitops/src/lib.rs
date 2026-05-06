use serde::{Deserialize, Serialize};

pub const SHADOW_REF_PREFIX: &str = "refs/iii/lineage/checkpoints/v0";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShadowSnapshotInput {
    pub session_id: String,
    pub parent_entry_id: Option<String>,
    pub repo_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShadowSnapshotOutput {
    pub shadow_ref: String,
    pub commit_oid: String,
    pub tree_oid: String,
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RewindInput {
    pub repo_path: String,
    pub shadow_ref: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttachTrailersInput {
    pub repo_path: String,
    pub commit_oid: String,
    pub session_id: String,
    pub entry_path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttachTrailersOutput {
    pub new_commit_oid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ListShadowRefsInput {
    pub repo_path: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShadowRefEntry {
    pub shadow_ref: String,
    pub session_id: String,
    pub entry_id: String,
    pub commit_oid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolveBlobInput {
    pub repo_path: String,
    pub shadow_ref: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolveBlobOutput {
    pub blob_oid: String,
    pub size: u64,
    pub content_base64: String,
}

pub mod ops;
pub mod registration;
