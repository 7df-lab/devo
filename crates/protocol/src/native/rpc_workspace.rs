//! Native `workspace/changes/read`.
//!
//! Params use Native session/turn ids. The read-model view type is
//! [`crate::WorkspaceChangeView`] (camelCase wire) — one type, no `From` dual.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::SessionId;
use super::ids::TurnId;
use crate::WorkspaceChangeScope;
use crate::WorkspaceChangeView;
use crate::WorkspaceDiffDetail;

// ── workspace/changes/read ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesReadParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub scopes: Vec<WorkspaceChangeScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub diff_detail: WorkspaceDiffDetail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_diff_bytes: Option<u64>,
    /// Server-side `--ignore-all-space` for the git-backed scopes
    /// (branch/staged/unstaged/uncommitted). Ignored for the turn scope,
    /// whose finalized diffs are precomputed artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_whitespace: Option<bool>,
    /// When set, Full (and Summary filtering) only considers these relative
    /// paths. Used by expand-on-demand so clients never wait on a whole-tree
    /// unified diff over stdio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<PathBuf>>,
    /// When true with path-scoped Full, attach per-file `oldText`/`newText`
    /// so clients can mount expandable MultiFileDiff views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_file_sides: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesReadResult {
    pub views: Vec<WorkspaceChangeView>,
}
