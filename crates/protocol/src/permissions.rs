use serde::Deserialize;
use serde::Serialize;

use crate::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum PermissionPreset {
    /// Read workspace files without approval; edits, commands, and network ask.
    ReadOnly,
    /// Read and edit workspace files and run shell commands; network and
    /// outside-workspace writes ask.
    #[default]
    Default,
    /// Same base policy as default, but eligible approvals may be routed
    /// through an automatic reviewer before the user is interrupted.
    AutoReview,
    /// Allow all tool requests without approval.
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalsReviewer {
    #[default]
    User,
    AutoReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPermissionsUpdateParams {
    pub session_id: SessionId,
    pub preset: PermissionPreset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPermissionsUpdateResult {
    pub session_id: SessionId,
    pub preset: PermissionPreset,
    pub reviewer: ApprovalsReviewer,
}
