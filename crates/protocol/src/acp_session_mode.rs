use serde::Deserialize;
use serde::Serialize;

use crate::acp::AcpMeta;

pub type AcpSessionModeId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionMode {
    pub id: AcpSessionModeId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionModeState {
    #[serde(rename = "availableModes")]
    pub available_modes: Vec<AcpSessionMode>,
    #[serde(rename = "currentModeId")]
    pub current_mode_id: AcpSessionModeId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}
