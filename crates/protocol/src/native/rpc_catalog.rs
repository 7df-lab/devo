//! Runtime catalog refresh. Remote directory policy belongs to the Native server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum CatalogRefreshPolicy {
    IfStale,
    Force,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogRefreshParams {
    pub policy: CatalogRefreshPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum CatalogRefreshStatus {
    Updated,
    Cached,
    Offline,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogRefreshResult {
    pub status: CatalogRefreshStatus,
    /// A failed refresh preserves the last good on-disk directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
