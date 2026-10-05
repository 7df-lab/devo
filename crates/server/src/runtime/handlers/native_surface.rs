//! Native methods that were registered in `NATIVE_METHODS` but previously
//! unrouted: `session/cwd/change`, `session/archive`, `turn/read`, `tool/list`.

use std::path::PathBuf;

use chrono::Utc;
use devo_protocol::SuccessResponse;
use devo_protocol::native::item::{Item, ItemEnvelope, ItemState, ToolSource};
use devo_protocol::native::rpc_admin::{ToolInfo, ToolListParams, ToolListResult};
use devo_protocol::native::rpc_session::{
    SessionArchiveParams, SessionArchiveResult, SessionCwdChangeParams, SessionCwdChangeResult,
};
use devo_protocol::native::rpc_turn::{TurnReadParams, TurnReadResult};

use super::super::*;
use crate::ProtocolErrorCode;

impl ServerRuntime {
    pub(crate) async fn handle_native_session_cwd_change(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionCwdChangeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/cwd/change params: {error}"),
                );
            }
        };
        let cwd = match validate_session_cwd(&params.cwd) {
            Ok(cwd) => cwd,
            Err(CwdError::Invalid) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!(
                        "cwd is not an accessible directory: {}",
                        params.cwd.display()
                    ),
                );
            }
            Err(CwdError::Denied) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::PermissionDenied,
                    format!("cwd is not readable: {}", params.cwd.display()),
                );
            }
        };
        let runtime_context = match self.deps.context_for_workspace(&cwd).await {
            Ok(context) => context,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to initialize session workspace: {error}"),
                );
            }
        };
        let handle = match self.get_or_load_parent_session(params.session_id).await {
            Ok(handle) => handle,
            Err(crate::runtime::session_cache::LoadSessionError::SessionLocked { holder_pid }) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionLocked,
                    format!("session is already open in another Devo process (pid {holder_pid})"),
                );
            }
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            }
        };
        if let Some(summary) = handle.summary().await
            && let Some(preset) = summary.permission_preset()
        {
            let safety_preset = match preset {
                devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
                devo_protocol::PermissionPreset::AutoReview => {
                    devo_safety::PermissionPreset::AutoReview
                }
                devo_protocol::PermissionPreset::FullAccess => {
                    devo_safety::PermissionPreset::FullAccess
                }
            };
            let profile =
                devo_safety::RuntimePermissionProfile::from_preset(safety_preset, cwd.clone())
                    .with_additional_roots(summary.additional_directories.clone());
            handle.apply_permission_profile(profile).await;
        }
        handle
            .update_session_workspace(cwd.clone(), runtime_context)
            .await;
        let Some(mut session) = handle.native_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        session.cwd = cwd.clone();
        session.version = session.version.saturating_add(1);
        if let Some(rollout_path) = handle.rollout_path().await.flatten()
            && let Err(error) =
                self.rollout_store
                    .append_session_meta_at(&rollout_path, &session, None)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session cwd change: {error}"),
            );
        }
        if let Some(summary) = handle.summary().await {
            self.persist_session_summary_if_persistent(params.session_id, &summary)
                .await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionCwdChanged {
                session_id: session.id,
                cwd: session.cwd.clone(),
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionCwdChangeResult { session },
        })
        .expect("serialize session/cwd/change response")
    }

    pub(crate) async fn handle_native_session_archive(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionArchiveParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/archive params: {error}"),
                );
            }
        };
        let handle = match self.get_or_load_parent_session(params.session_id).await {
            Ok(handle) => handle,
            Err(crate::runtime::session_cache::LoadSessionError::SessionLocked { holder_pid }) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionLocked,
                    format!("session is already open in another Devo process (pid {holder_pid})"),
                );
            }
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            }
        };
        let Some(session) = handle.set_archived(params.archived).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if let Some(rollout_path) = handle.rollout_path().await.flatten()
            && let Err(error) =
                self.rollout_store
                    .append_session_meta_at(&rollout_path, &session, None)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session archive: {error}"),
            );
        }
        if let Some(summary) = handle.summary().await {
            self.persist_session_summary_if_persistent(params.session_id, &summary)
                .await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionArchived {
                session_id: session.id,
                archived: session.archived,
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionArchiveResult { session },
        })
        .expect("serialize session/archive response")
    }

    pub(crate) async fn handle_native_turn_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnReadParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/read params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        let Some(turn) = history
            .turns
            .iter()
            .find(|turn| turn.id == params.turn_id)
            .cloned()
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn does not exist",
            );
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnReadResult { turn },
        })
        .expect("serialize turn/read response")
    }

    pub(crate) async fn handle_native_tool_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ToolListParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid tool/list params: {error}"),
                );
            }
        };
        let registry = if let Some(session_id) = params.session_id {
            match self.session(session_id).await {
                Some(handle) => handle
                    .runtime_context()
                    .await
                    .map(|context| context.tool_registry())
                    .unwrap_or_else(|| self.deps.process_context.tool_registry()),
                None => self.deps.process_context.tool_registry(),
            }
        } else {
            self.deps.process_context.tool_registry()
        };
        let tools = registry
            .tool_definitions()
            .into_iter()
            .map(|definition| ToolInfo {
                name: definition.name,
                source: ToolSource::Builtin,
                server_name: None,
                description: Some(definition.description),
            })
            .collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ToolListResult { tools },
        })
        .expect("serialize tool/list response")
    }
}

enum CwdError {
    Invalid,
    Denied,
}

fn validate_session_cwd(cwd: &std::path::Path) -> Result<PathBuf, CwdError> {
    let normalized = devo_core::normalize_native_path(cwd.to_path_buf());
    match std::fs::metadata(&normalized) {
        Ok(metadata) if metadata.is_dir() => Ok(normalized),
        Ok(_) => Err(CwdError::Invalid),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(CwdError::Denied),
        Err(_) => Err(CwdError::Invalid),
    }
}

pub(super) fn live_item_snapshot(
    session_id: devo_protocol::native::ids::SessionId,
    turn_id: devo_protocol::native::ids::TurnId,
    item_id: devo_protocol::native::ids::ItemId,
    seq: u64,
    item: Item,
    channel: devo_protocol::native::event::DeltaChannel,
    text: String,
) -> devo_protocol::native::event::LiveItemSnapshot {
    let now = Utc::now();
    devo_protocol::native::event::LiveItemSnapshot {
        item: ItemEnvelope {
            id: item_id,
            session_id,
            turn_id,
            seq,
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Running,
            item,
            parent_id: None,
        },
        accumulated: vec![devo_protocol::native::event::ChannelAccumulation {
            channel,
            text,
            next_chunk_index: 0,
        }],
    }
}
