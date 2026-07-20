use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSandboxProfileUpdateParams {
    pub session_id: SessionId,
    /// Built-in profile name (`workspace`, `devbox`, `read-only`, `strict`,
    /// `off`) or a custom profile defined in `sandbox.toml`.
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSandboxProfileUpdateResult {
    pub session_id: SessionId,
    /// Canonical profile name now active for the session.
    pub profile: String,
}
