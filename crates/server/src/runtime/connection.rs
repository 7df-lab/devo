use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::ACP_AUTHENTICATE_METHOD;
use crate::ACP_INITIALIZE_METHOD;
use crate::ACP_LOGOUT_METHOD;
use crate::acp_auth_required_response;
use crate::acp_notification_from_server_notification;
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::notification_bus::{
    notification_legacy_session_id, notification_method_name, notification_touches_session_activity,
};
use devo_protocol::native::wire_projector::wire_from_server_notification;

use super::handlers::acp::AcpRoute;
use super::handlers::acp::acp_route;
use super::outbound::OutboundDeliveryPolicy;
use super::outbound::OutboundFrame;
use super::outbound::enqueue_outbound;
use super::outbound::enqueue_outbound_notification;

pub(crate) const INBOUND_CONCURRENCY_LIMIT: usize = 64;

#[derive(Debug)]
pub struct IncomingResponse {
    response: serde_json::Value,
    post_response_actions: PostResponseActions,
}

impl IncomingResponse {
    fn new(response: serde_json::Value) -> Self {
        Self {
            response,
            post_response_actions: PostResponseActions::default(),
        }
    }

    fn with_post_response_action(mut self, action: PostResponseAction) -> Self {
        self.post_response_actions.0.push(action);
        self
    }

    pub fn into_parts(self) -> (serde_json::Value, PostResponseActions) {
        (self.response, self.post_response_actions)
    }

    fn is_success(&self) -> bool {
        self.response.get("result").is_some() && self.response.get("error").is_none()
    }

    fn with_acp_session_state_snapshot_after_success(
        self,
        connection_id: u64,
        session_id: Option<SessionId>,
    ) -> Self {
        let Some(session_id) = session_id else {
            return self;
        };
        if !self.is_success() {
            return self;
        }
        self.with_post_response_action(PostResponseAction::SendAcpSessionStateSnapshot {
            connection_id,
            session_id,
        })
    }
}

#[derive(Debug, Default)]
pub struct PostResponseActions(Vec<PostResponseAction>);

#[derive(Debug)]
enum PostResponseAction {
    SendAcpSessionStateSnapshot {
        connection_id: u64,
        session_id: SessionId,
    },
}

/// Which protocol surface a connection is bound to, negotiated at
/// `initialize` via `_meta.devo.protocol` (L2-DES-APP-008 / L2-DES-APP-009).
///
/// ACP and the native protocol share method names (`session/new`,
/// `session/resume`, `session/list`, `session/delete`, …), so routing cannot
/// key on the method name alone: ACP connections keep the ACP adapter
/// behavior pinned by `protocol-lock.json`, native connections route
/// those names to the native handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionProtocol {
    /// ACP adapter surface.
    Acp,
    /// Native protocol surface for first-party clients.
    Native,
}

impl ServerRuntime {
    pub async fn register_connection(
        self: &Arc<Self>,
        transport: ClientTransportKind,
        outbound_tx: mpsc::Sender<OutboundFrame>,
    ) -> u64 {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::SeqCst);
        let mut connections = self.connections.lock().await;
        connections.insert(
            connection_id,
            ConnectionRuntime {
                transport,
                state: ConnectionState::Connected,
                protocol: None,
                acp_authenticated: false,
                acp_client_capabilities: crate::AcpClientCapabilities::default(),
                typed_items: false,
                event_selectors: Vec::new(),
                outbound_tx,
                opt_out_notification_methods: HashSet::new(),
                subscriptions: Vec::new(),
                next_event_seq: 1,
                next_client_request_id: 1,
                pending_client_requests: HashMap::new(),
            },
        );
        tracing::info!(
            connection_id,
            transport = ?connections
                .get(&connection_id)
                .map(|connection| connection.transport.clone())
                .expect("connection inserted"),
            active_connections = connections.len(),
            "registered client connection"
        );
        connection_id
    }

    pub async fn unregister_connection(&self, connection_id: u64) {
        let mut connections = self.connections.lock().await;
        let mut removed = connections.remove(&connection_id);
        drop(connections);
        if let Some(connection) = removed.as_mut() {
            for (_, pending) in connection.pending_client_requests.drain() {
                let _ = pending.send(Err("client connection closed".to_string()));
            }
        }
        self.drop_restore_plans_for_connection(connection_id).await;
        self.active_turns.drop_connection_id(connection_id).await;
        self.drop_event_subscriptions_for_connection(connection_id)
            .await;
        self.reference_searches
            .lock()
            .await
            .retain(|_, state| state.connection_id() != connection_id);
        self.command_exec_manager
            .terminate_connection(connection_id)
            .await;
        let active_connections = self.connections.lock().await.len();
        tracing::info!(
            connection_id,
            transport = ?removed.as_ref().map(|connection| connection.transport.clone()),
            active_connections,
            "unregistered client connection"
        );
    }

    /// Number of live client connections (stdio + internal stdio proxies).
    pub async fn active_connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    pub async fn handle_incoming(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let (response, post_response_actions) = self
            .handle_incoming_with_actions(connection_id, message)
            .await?
            .into_parts();
        self.run_post_response_actions(post_response_actions).await;
        Some(response)
    }

    pub async fn handle_incoming_with_actions(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) -> Option<IncomingResponse> {
        if message.get("method").is_none()
            && message.get("id").is_some()
            && (message.get("result").is_some() || message.get("error").is_some())
        {
            self.resolve_pending_client_response(connection_id, message)
                .await;
            return None;
        }
        let method = message.get("method")?.as_str()?.to_string();
        let id = message.get("id").cloned();
        let params = message
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        tracing::debug!(
            connection_id,
            method,
            has_id = id.is_some(),
            "received client message"
        );

        if method == ACP_INITIALIZE_METHOD {
            return Some(IncomingResponse::new(
                self.handle_acp_initialize(connection_id, id, params).await,
            ));
        }
        // Before connection enter `Ready` state, only allowed method: "initialize"
        if !self.connection_ready(connection_id).await {
            return id.map(|request_id| {
                IncomingResponse::new(self.error_response(
                    request_id,
                    ProtocolErrorCode::NotInitialized,
                    "connection has not completed initialize",
                ))
            });
        }

        let protocol = self
            .connection_protocol(connection_id)
            .await
            .expect("ready connection must have a negotiated protocol");
        if protocol == ConnectionProtocol::Acp {
            if method == ACP_AUTHENTICATE_METHOD {
                return Some(IncomingResponse::new(
                    self.handle_acp_authenticate(connection_id, id, params)
                        .await,
                ));
            }
            if method == ACP_LOGOUT_METHOD {
                return Some(IncomingResponse::new(
                    self.handle_acp_logout(connection_id, id, params).await,
                ));
            }
            if !self.connection_authenticated(connection_id).await {
                if let Some(request_id) = id {
                    return Some(IncomingResponse::new(acp_auth_required_response(
                        request_id,
                    )));
                }
                tracing::warn!(
                    connection_id,
                    method,
                    "dropping unauthenticated ACP client notification"
                );
                return None;
            }
            if let Some(route) = acp_route(&method) {
                return self
                    .handle_acp_route(route, connection_id, id, params)
                    .await;
            }
            return id.map(|request_id| {
                IncomingResponse::new(crate::acp_error_response(
                    request_id,
                    crate::AcpErrorCode::MethodNotFound,
                    format!("unknown ACP method: {method}"),
                ))
            });
        }

        let response = match method.as_str() {
            // Update session metadata, including the current model and reasoning effort.
            "session/metadata/update" => Some(
                self.handle_native_session_metadata_update(id?, params)
                    .await,
            ),
            // resume a history session, server load the jsonl file then replay the events in jsonl
            "session/resume" => Some(
                self.handle_native_session_resume(connection_id, id?, params)
                    .await,
            ),
            // fork a given session at given user turn index
            "session/fork" => Some(
                self.handle_native_session_fork(connection_id, id?, params)
                    .await,
            ),
            "session/rollback/preview" => Some(
                self.handle_session_rollback_preview(connection_id, id?, params)
                    .await,
            ),
            "session/rollback/commit" => Some(
                self.handle_session_rollback_commit(connection_id, id?, params)
                    .await,
            ),
            // compact session context history
            "session/compact/start" => {
                Some(self.handle_native_session_compact_start(id?, params).await)
            }
            "session/new" => Some(
                self.handle_native_session_new(connection_id, id?, params)
                    .await,
            ),
            "session/list" => Some(self.handle_native_session_list(id?, params).await),
            "session/read" => Some(self.handle_native_session_read(id?, params).await),
            "session/systemPrompt/read" => Some(
                self.handle_native_session_system_prompt_read(id?, params)
                    .await,
            ),
            "session/schedule/list" => {
                Some(self.handle_native_session_schedule_list(id?, params).await)
            }
            "session/schedule/upsert" => Some(
                self.handle_native_session_schedule_upsert(id?, params)
                    .await,
            ),
            "session/schedule/update" => Some(
                self.handle_native_session_schedule_update(id?, params)
                    .await,
            ),
            "session/schedule/delete" => Some(
                self.handle_native_session_schedule_delete(id?, params)
                    .await,
            ),
            "session/heartbeat/command" => Some(
                self.handle_native_session_heartbeat_command(id?, params)
                    .await,
            ),
            "session/export" => Some(self.handle_native_session_export(id?, params).await),
            "session/import" => Some(
                self.handle_native_session_import(connection_id, id?, params)
                    .await,
            ),
            "session/interrupt" => Some(
                self.handle_session_interrupt(connection_id, id?, params)
                    .await,
            ),
            "runtime/ping" => Some(
                serde_json::to_value(SuccessResponse {
                    id: id?,
                    result: devo_protocol::native::rpc_admin::RuntimePingResult {
                        server_time_ms: Utc::now().timestamp_millis(),
                    },
                })
                .expect("serialize runtime/ping response"),
            ),
            "model/list" => Some(self.handle_native_model_list(id?, params).await),
            "model/preferences/read" => {
                Some(self.handle_native_model_preferences_read(id?, params).await)
            }
            "model/preferences/write" => Some(
                self.handle_native_model_preferences_write(id?, params)
                    .await,
            ),
            "session/delete" => Some(self.handle_native_session_delete(id?, params).await),
            "session/cwd/change" => Some(self.handle_native_session_cwd_change(id?, params).await),
            "session/archive" => Some(self.handle_native_session_archive(id?, params).await),
            "turn/read" => Some(self.handle_native_turn_read(id?, params).await),
            "tool/list" => Some(self.handle_native_tool_list(id?, params).await),
            "session/goal/set" => Some(self.handle_native_session_goal_set(id?, params).await),
            "session/refine/run" => Some(self.handle_native_session_refine_run(id?, params).await),
            "session/goal/read" => Some(self.handle_native_session_goal_read(id?, params).await),
            "session/goal/update" => {
                Some(self.handle_native_session_goal_update(id?, params).await)
            }
            "session/message/edit" => Some(
                self.handle_native_session_message_edit(connection_id, id?, params)
                    .await,
            ),
            "task/start" => Some(
                self.handle_native_task_start(connection_id, id?, params)
                    .await,
            ),
            "task/read" => Some(self.handle_native_task_read(id?, params).await),
            "task/list" => Some(self.handle_native_task_list(id?, params).await),
            "task/interrupt" => Some(
                self.handle_native_task_interrupt(connection_id, id?, params)
                    .await,
            ),
            "task/write_stdin" => Some(
                self.handle_native_task_write_stdin(connection_id, id?, params)
                    .await,
            ),
            "task/resize" => Some(
                self.handle_native_task_resize(connection_id, id?, params)
                    .await,
            ),
            "agent/cancel" => Some(self.handle_native_agent_cancel(id?, params).await),
            "agent/list" => Some(self.handle_native_agent_list(id?, params).await),
            "agent/message" => Some(self.handle_native_agent_message(id?, params).await),
            "agent/read" => Some(self.handle_native_agent_read(id?, params).await),
            "session/goal/pause"
            | "session/goal/resume"
            | "session/goal/complete"
            | "session/goal/cancel"
            | "session/goal/clear" => Some(
                self.handle_native_session_goal_transition(&method, id?, params)
                    .await,
            ),
            "skill/list" => Some(self.handle_native_skill_list(id?, params).await),
            "skill/set_enabled" => Some(self.handle_native_skill_set_enabled(id?, params).await),
            "mcp/list" => Some(self.handle_mcp_list(id?, params).await),
            "mcp/tools" => Some(self.handle_mcp_tools(id?, params).await),
            "mcp/set_enabled" => Some(self.handle_mcp_set_enabled(id?, params).await),
            "context/usage/read" => Some(self.handle_context_usage_read(id?, params).await),
            "search/start" => Some(
                self.handle_native_search_start(connection_id, id?, params)
                    .await,
            ),
            "search/update" => Some(self.handle_native_search_update(id?, params).await),
            "search/cancel" => Some(self.handle_native_search_cancel(id?, params).await),
            "command/exec" => Some(self.handle_command_exec(connection_id, id?, params).await),
            "command/exec/write" => Some(
                self.handle_command_exec_write(connection_id, id?, params)
                    .await,
            ),
            "command/exec/resize" => Some(
                self.handle_command_exec_resize(connection_id, id?, params)
                    .await,
            ),
            "command/exec/terminate" => Some(
                self.handle_command_exec_terminate(connection_id, id?, params)
                    .await,
            ),
            "turn/resume" => Some(self.handle_turn_resume(connection_id, id?, params).await),
            "turn/recovery/read" => Some(self.handle_turn_recovery_read(id?, params).await),
            "turn/start" => Some(
                self.handle_turn_start_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            "turn/steer" => Some(self.handle_turn_steer(connection_id, id?, params).await),
            "workspace/changes/read" => Some(self.handle_workspace_changes_read(id?, params).await),
            "provider/list" => Some(self.handle_native_provider_list(id?).await),
            "provider/validate" => Some(self.handle_native_provider_validate(id?, params).await),
            "provider/discover" => Some(self.handle_native_provider_discover(id?, params).await),
            "provider/upsert" => Some(self.handle_native_provider_upsert(id?, params).await),
            "provider/disconnect" => {
                Some(self.handle_native_provider_disconnect(id?, params).await)
            }
            "provider/model/remove" => {
                Some(self.handle_native_provider_model_remove(id?, params).await)
            }
            "credential/list" => Some(self.handle_native_credential_list(id?).await),
            "credential/set" => Some(self.handle_native_credential_set(id?, params).await),
            "credential/delete" => Some(self.handle_native_credential_delete(id?, params).await),
            // Paged history reads of the new Native API (native types).
            "session/turns/list" => Some(self.handle_session_turns_list(id?, params).await),
            "session/items/list" => Some(self.handle_session_items_list(id?, params).await),
            "session/tree/read" => Some(self.handle_session_tree_read(id?, params).await),
            "session/tree/navigate" => Some(self.handle_session_tree_navigate(id?, params).await),
            // Durable event subscriptions (08 §4).
            "subscription/create" => Some(
                self.handle_subscription_create(connection_id, id?, params)
                    .await,
            ),
            "subscription/update" => Some(
                self.handle_subscription_update(connection_id, id?, params)
                    .await,
            ),
            "subscription/ack" => Some(
                self.handle_subscription_ack(connection_id, id?, params)
                    .await,
            ),
            "subscription/unsubscribe" => Some(
                self.handle_subscription_unsubscribe(connection_id, id?, params)
                    .await,
            ),
            // Session input queue of the new Native API (01 §4.3).
            "session/queue/push" => Some(
                self.handle_session_queue_push(connection_id, id?, params)
                    .await,
            ),
            "session/queue/list" => Some(self.handle_session_queue_list(id?, params).await),
            "session/queue/update" => Some(self.handle_session_queue_update(id?, params).await),
            "session/queue/remove" => Some(self.handle_session_queue_remove(id?, params).await),
            _ => Some(self.error_response(
                id?,
                ProtocolErrorCode::InvalidParams,
                format!("unknown method: {method}"),
            )),
        };
        // Filter out responses already dispatched via the high-priority channel.
        match response {
            Some(serde_json::Value::Null) => None,
            Some(response) => Some(IncomingResponse::new(response)),
            None => None,
        }
    }

    async fn handle_acp_route(
        self: &Arc<Self>,
        route: AcpRoute,
        connection_id: u64,
        request_id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> Option<IncomingResponse> {
        match route {
            AcpRoute::Cancel => {
                self.handle_acp_session_cancel(params).await;
                Some(IncomingResponse::new(crate::acp_success_response(
                    request_id?,
                    crate::AcpEmptyResult::default(),
                )))
            }
            AcpRoute::SessionInterrupt => Some(IncomingResponse::new(
                self.handle_session_interrupt(connection_id, request_id?, params)
                    .await,
            )),
            AcpRoute::Close => Some(IncomingResponse::new(
                self.handle_acp_session_close(request_id?, params).await,
            )),
            AcpRoute::Delete => Some(IncomingResponse::new(
                self.handle_acp_session_delete(request_id?, params).await,
            )),
            AcpRoute::List => Some(IncomingResponse::new(
                self.handle_acp_session_list(request_id?, params).await,
            )),
            AcpRoute::Load => {
                let session_id =
                    serde_json::from_value::<crate::AcpLoadSessionParams>(params.clone())
                        .ok()
                        .map(|params| params.session_id);
                let response = IncomingResponse::new(
                    self.handle_acp_session_load(connection_id, request_id?, params)
                        .await,
                );
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::New => {
                let response = IncomingResponse::new(
                    self.handle_acp_session_new(connection_id, request_id?, params)
                        .await,
                );
                let session_id = serde_json::from_value::<
                    crate::AcpSuccessResponse<crate::AcpNewSessionResult>,
                >(response.response.clone())
                .ok()
                .map(|response| response.result.session_id);
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::Prompt => self
                .handle_acp_session_prompt(connection_id, request_id?, params)
                .await
                .map(IncomingResponse::new),
            AcpRoute::Resume => {
                let session_id =
                    serde_json::from_value::<crate::AcpResumeSessionParams>(params.clone())
                        .ok()
                        .map(|params| params.session_id);
                let response = IncomingResponse::new(
                    self.handle_acp_session_resume(connection_id, request_id?, params)
                        .await,
                );
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::SetConfigOption => Some(IncomingResponse::new(
                self.handle_acp_session_set_config_option(request_id?, params)
                    .await,
            )),
            AcpRoute::SetMode => Some(IncomingResponse::new(
                self.handle_acp_session_set_mode(request_id?, params).await,
            )),
        }
    }

    pub async fn run_post_response_actions(self: &Arc<Self>, actions: PostResponseActions) {
        for action in actions.0 {
            match action {
                PostResponseAction::SendAcpSessionStateSnapshot {
                    connection_id,
                    session_id,
                } => {
                    self.send_acp_session_state_snapshot(connection_id, session_id)
                        .await;
                }
            }
        }
    }

    pub(super) async fn subscribe_connection_to_session(
        &self,
        connection_id: u64,
        session_id: SessionId,
        event_types: Option<HashSet<String>>,
    ) {
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            let desired = event_types.unwrap_or_default();
            let already = connection.subscriptions.iter().any(|subscription| {
                subscription.session_id == Some(session_id) && subscription.event_types == desired
            });
            if already {
                return;
            }
            let include_child_agents = matches!(
                connection.transport,
                ClientTransportKind::Stdio | ClientTransportKind::StdioProxy
            );
            connection.subscriptions.push(SubscriptionFilter {
                session_id: Some(session_id),
                event_types: desired,
                include_child_agents,
            });
        }
    }

    pub(super) async fn connection_ready(&self, connection_id: u64) -> bool {
        self.connections
            .lock()
            .await
            .get(&connection_id)
            .is_some_and(|connection| connection.state == ConnectionState::Ready)
    }

    pub(super) async fn connection_protocol(
        &self,
        connection_id: u64,
    ) -> Option<ConnectionProtocol> {
        self.connections
            .lock()
            .await
            .get(&connection_id)
            .and_then(|connection| connection.protocol)
    }

    pub async fn resolve_client_response(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) {
        self.resolve_pending_client_response(connection_id, message)
            .await;
    }

    /// Hot path for child/parent assistant token streaming.
    ///
    /// Avoids per-token `child_parent_by_session` registry scans and never waits
    /// on the wait_agent output buffer. Uses `active_turn_connections` to find
    /// the owning stdio connection directly.
    pub(super) async fn broadcast_streaming_agent_message_delta(
        &self,
        notification: &ServerNotification,
    ) {
        let ServerNotification::ItemAssistantMessageDelta(delta) = notification else {
            self.broadcast_notification(notification.clone()).await;
            return;
        };
        let session_id = SessionId::from(delta.session_id.as_str());
        let Some(connection_id) = self.active_turns.active_connection_id(session_id).await else {
            self.broadcast_notification(notification.clone()).await;
            return;
        };
        let (wire_method, wire_params) = wire_from_server_notification(notification);
        let frames = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if connection
                .opt_out_notification_methods
                .contains(wire_method.as_str())
            {
                return;
            }
            connection.prepare_notification_outbound(
                connection_id,
                wire_method,
                wire_params,
                notification,
            )
        };
        self.deliver_notification_frames(frames, OutboundDeliveryPolicy::BestEffort)
            .await;
        self.record_subagent_output_notification(notification).await;
    }

    /// Deliver a connection-local notification to one client connection.
    ///
    /// Connection-local search notifications (`search/updated`, `search/completed`,
    /// `search/failed`) are not session transcript events. They carry no
    /// `session_id` and must reach the requesting connection even when the only
    /// active subscriptions are session-scoped (`session_id=Some(...)`).
    pub(super) async fn emit_connection_local_notification(
        &self,
        connection_id: u64,
        notification: ServerNotification,
    ) {
        let method = notification_method_name(&notification);
        debug_assert!(is_connection_local_notification(method.as_str()));
        let delivery_policy = notification_delivery_policy(&notification);
        let (wire_method, wire_params) = wire_from_server_notification(&notification);
        let frames = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if !connection.should_deliver_connection_local(method.as_str()) {
                return;
            }
            connection.prepare_notification_outbound(
                connection_id,
                wire_method,
                wire_params,
                &notification,
            )
        };
        self.deliver_notification_frames(frames, delivery_policy)
            .await;
    }

    /// Deliver one Native notification to a single connection (owner-routed).
    pub(super) async fn emit_notification_to_connection(
        &self,
        connection_id: u64,
        notification: ServerNotification,
    ) {
        let method = notification_method_name(&notification);
        if is_connection_local_notification(method.as_str()) {
            self.emit_connection_local_notification(connection_id, notification)
                .await;
            return;
        }
        let session_id = notification_legacy_session_id(&notification);
        let delivery_policy = notification_delivery_policy(&notification);
        let (wire_method, wire_params) = wire_from_server_notification(&notification);
        let child_parent_by_session = self.child_parent_by_session().await;
        let frames = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if !connection.should_deliver(method.as_str(), session_id, &child_parent_by_session) {
                return;
            }
            connection.prepare_notification_outbound(
                connection_id,
                wire_method,
                wire_params,
                &notification,
            )
        };
        self.deliver_notification_frames(frames, delivery_policy)
            .await;
    }

    /// First-party bus fan-out for Native-covered lifecycle notifications.
    ///
    /// Native connections receive identity wire frames (`item/*`, `turn/*`,
    /// deltas). ACP projects from the same [`ServerNotification`].
    pub(super) async fn broadcast_notification(&self, notification: ServerNotification) {
        if let ServerNotification::TurnCompleted { turn } = &notification {
            self.account_goal_turn_completed(turn).await;
        }
        self.update_session_last_activity_from_notification(&notification)
            .await;
        let method = notification_method_name(&notification);
        let session_id = notification_legacy_session_id(&notification);
        let delivery_policy = notification_delivery_policy(&notification);
        let (wire_method, wire_params) = wire_from_server_notification(&notification);
        let child_parent_by_session = self.child_parent_by_session().await;
        let event_cwd = if self
            .sessions_by_cwd_subscriptions
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
            && let Some(session_id) = session_id
        {
            match self.session(session_id).await {
                Some(handle) => handle.summary().await.map(|summary| summary.cwd.clone()),
                None => None,
            }
        } else {
            None
        };
        let frames_out = {
            let mut connections = self.connections.lock().await;
            connections
                .iter_mut()
                .filter_map(|(connection_id, connection)| {
                    if !connection.should_deliver(
                        method.as_str(),
                        session_id,
                        &child_parent_by_session,
                    ) && !crate::runtime::handlers::subscription::notification_matches_selectors(
                        &connection.event_selectors,
                        &notification,
                        event_cwd.as_deref(),
                    ) {
                        return None;
                    }
                    Some(connection.prepare_notification_outbound(
                        *connection_id,
                        wire_method.clone(),
                        wire_params.clone(),
                        &notification,
                    ))
                })
                .flatten()
                .collect::<Vec<_>>()
        };
        self.deliver_notification_frames(frames_out, delivery_policy)
            .await;
        self.record_subagent_output_notification(&notification)
            .await;
    }

    async fn deliver_notification_frames(
        &self,
        frames: Vec<(mpsc::Sender<OutboundFrame>, OutboundFrame)>,
        delivery_policy: OutboundDeliveryPolicy,
    ) {
        for (outbound_tx, frame) in frames {
            enqueue_outbound_notification(
                &outbound_tx,
                frame,
                delivery_policy,
                "connection_notifications",
            )
            .await;
        }
    }

    async fn update_session_last_activity_from_notification(
        &self,
        notification: &ServerNotification,
    ) {
        if !notification_touches_session_activity(notification) {
            return;
        }
        let Some(session_id) = notification_legacy_session_id(notification) else {
            return;
        };
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                inline.summary.last_activity_at = inline.summary.last_activity_at.max(Utc::now());
                return;
            }
        }
        if let Some(session_handle) = self.session(session_id).await {
            let _ = session_handle.try_touch_last_activity();
        }
    }

    pub(super) async fn send_raw_to_connection(
        &self,
        connection_id: u64,
        value: serde_json::Value,
    ) {
        let (outbound_tx, frame) = {
            let connections = self.connections.lock().await;
            let Some(connection) = connections.get(&connection_id) else {
                return;
            };
            let (delivered_tx, delivered_rx) = oneshot::channel();
            (
                connection.outbound_tx.clone(),
                (
                    OutboundFrame::json_rpc_response_with_delivery(
                        connection_id,
                        value,
                        delivered_tx,
                    ),
                    delivered_rx,
                ),
            )
        };
        let (frame, delivered_rx) = frame;
        if !enqueue_outbound(&outbound_tx, frame, "connection_responses").await {
            return;
        }
        let _ = delivered_rx.await;
    }

    pub(super) async fn send_request_to_connection_cancellable_with_enqueue_signal(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        cancel_token: CancellationToken,
        enqueued_tx: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            /*timeout_duration*/ None,
            cancel_token,
            Some(enqueued_tx),
        )
        .await
    }

    pub(super) async fn send_request_to_connection_with_timeout(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        timeout_duration: Duration,
        cancel_token: CancellationToken,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            Some(timeout_duration),
            cancel_token,
            /*enqueued_tx*/ None,
        )
        .await
    }

    async fn send_request_to_connection_inner(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        timeout_duration: Option<Duration>,
        cancel_token: CancellationToken,
        enqueued_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    ) -> Result<serde_json::Value, String> {
        let (request_id, receiver, outbound_tx, frame) = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return Err("client connection does not exist".to_string());
            };
            let request_id = connection.next_client_request_id;
            connection.next_client_request_id += 1;
            let (tx, rx) = oneshot::channel();
            connection.pending_client_requests.insert(request_id, tx);
            let value = serde_json::to_value(devo_protocol::AcpClientRequest::new(
                serde_json::json!(request_id),
                method,
                params,
            ))
            .map_err(|error| format!("failed to serialize client request: {error}"))?;
            (
                request_id,
                rx,
                connection.outbound_tx.clone(),
                OutboundFrame::client_request(connection_id, method.to_string(), value),
            )
        };
        let mut pending_request = PendingClientRequestGuard::new(
            Arc::clone(&self.connections),
            connection_id,
            request_id,
        );
        if !enqueue_outbound(&outbound_tx, frame, "connection_requests").await {
            pending_request.remove().await;
            return Err("client connection closed before request was sent".to_string());
        }
        if let Some(enqueued_tx) = enqueued_tx {
            let _ = enqueued_tx.send(());
        }
        let message = match timeout_duration {
            Some(timeout_duration) => {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        pending_request.remove().await;
                        return Err("client request cancelled".to_string());
                    }
                    result = tokio::time::timeout(timeout_duration, receiver) => {
                        match result {
                            Ok(Ok(message)) => {
                                pending_request.disarm();
                                message?
                            }
                            Ok(Err(_)) => {
                                pending_request.disarm();
                                return Err("client connection closed before responding".to_string());
                            }
                            Err(_) => {
                                pending_request.remove().await;
                                return Err(format!(
                                    "client request timed out after {}s",
                                    timeout_duration.as_secs()
                                ));
                            }
                        }
                    }
                }
            }
            None => {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        pending_request.remove().await;
                        return Err("client request cancelled".to_string());
                    }
                    result = receiver => {
                        pending_request.disarm();
                        result.map_err(|_| "client connection closed before responding".to_string())??
                    }
                }
            }
        };
        if let Some(error) = message.get("error") {
            return Err(error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("client returned an error response")
                .to_string());
        }
        Ok(message
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    async fn resolve_pending_client_response(
        &self,
        connection_id: u64,
        message: serde_json::Value,
    ) {
        let Some(request_id) = message.get("id").and_then(serde_json::Value::as_u64) else {
            tracing::warn!(connection_id, "dropping client response with non-u64 id");
            return;
        };
        let pending = {
            let mut connections = self.connections.lock().await;
            connections
                .get_mut(&connection_id)
                .and_then(|connection| connection.pending_client_requests.remove(&request_id))
        };
        if let Some(pending) = pending {
            let _ = pending.send(Ok(message));
        } else {
            tracing::warn!(
                connection_id,
                request_id,
                "dropping response for unknown server-initiated request"
            );
        }
    }

    pub(super) fn error_response(
        &self,
        request_id: serde_json::Value,
        code: ProtocolErrorCode,
        message: impl Into<String>,
    ) -> serde_json::Value {
        let message = message.into();
        tracing::warn!(
            request_id = %request_id,
            code = ?code,
            error_message = %message,
            "returning protocol error"
        );
        serde_json::to_value(ErrorResponse {
            id: request_id,
            error: ProtocolError {
                code,
                message,
                data: serde_json::json!({}),
            },
        })
        .expect("serialize error response")
    }
}

impl ServerRuntime {
    pub(super) async fn controlling_connection_ids(
        &self,
        session_id: SessionId,
        owner_connection_id: Option<u64>,
    ) -> Vec<u64> {
        let native_session_id = self
            .session_summary_snapshot(session_id)
            .await
            .map(|summary| summary.native.id)
            .unwrap_or_else(|| {
                // boundary: session not loaded; legacy map key only
                session_id
            });
        let connections = self.connections.lock().await;
        let mut connection_ids = connections
            .iter()
            .filter_map(|(connection_id, connection)| {
                let subscribed = connection.event_selectors.iter().any(|selector| {
                    matches!(
                        selector,
                        devo_protocol::native::event::StreamSelector::Session {
                            session_id
                        } if session_id == &native_session_id
                    )
                });
                (subscribed || Some(*connection_id) == owner_connection_id)
                    .then_some(*connection_id)
            })
            .collect::<Vec<_>>();
        connection_ids.sort_unstable();
        connection_ids
    }

    async fn child_parent_by_session(&self) -> HashMap<SessionId, SessionId> {
        self.agent_registries
            .lock()
            .await
            .values()
            .flat_map(|registry| {
                registry
                    .child_to_parent
                    .iter()
                    .map(|(child, parent)| (*child, *parent))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

struct PendingClientRequestGuard {
    connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
    connection_id: u64,
    request_id: u64,
    active: bool,
}

impl PendingClientRequestGuard {
    fn new(
        connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
        connection_id: u64,
        request_id: u64,
    ) -> Self {
        Self {
            connections,
            connection_id,
            request_id,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }

    async fn remove(&mut self) {
        if !self.active {
            return;
        }
        remove_pending_client_request(&self.connections, self.connection_id, self.request_id).await;
        self.active = false;
    }
}

impl Drop for PendingClientRequestGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let connections = Arc::clone(&self.connections);
        let connection_id = self.connection_id;
        let request_id = self.request_id;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                remove_pending_client_request(&connections, connection_id, request_id).await;
            });
        }
    }
}

async fn remove_pending_client_request(
    connections: &Mutex<HashMap<u64, ConnectionRuntime>>,
    connection_id: u64,
    request_id: u64,
) {
    if let Some(connection) = connections.lock().await.get_mut(&connection_id) {
        connection.pending_client_requests.remove(&request_id);
    }
}

fn notification_delivery_policy(notification: &ServerNotification) -> OutboundDeliveryPolicy {
    match notification {
        ServerNotification::ItemAssistantMessageDelta(_)
        | ServerNotification::ItemReasoningDelta(_)
        | ServerNotification::ItemCommandExecutionOutputDelta(_)
        | ServerNotification::ItemPlanDelta(_)
        | ServerNotification::TurnUsageUpdated { .. }
        | ServerNotification::ContextUsageUpdated { .. }
        | ServerNotification::WorkspaceChangesUpdated(_)
        | ServerNotification::SearchUpdated(_)
        | ServerNotification::CommandExecOutputDelta { .. } => OutboundDeliveryPolicy::BestEffort,
        ServerNotification::ItemToolCallInputDelta(_) => OutboundDeliveryPolicy::Reliable,
        _ => OutboundDeliveryPolicy::Reliable,
    }
}

pub(crate) struct ConnectionRuntime {
    pub(crate) transport: ClientTransportKind,
    pub(crate) state: ConnectionState,
    /// Negotiated protocol surface; gates ACP route dispatch.
    pub(crate) protocol: Option<ConnectionProtocol>,
    pub(crate) acp_authenticated: bool,
    pub(crate) acp_client_capabilities: crate::AcpClientCapabilities,
    /// Whether the client opted in to native typed `item/*` notifications
    /// via `_meta.devo.typedItems` on ACP initialize (P2).
    pub(crate) typed_items: bool,
    /// Cached union of this connection's new-style (`subscription/*`)
    /// selector sets; rebuilt on every create/update/unsubscribe. Delivery
    /// reads only this cache (the registry is authoritative for ack state).
    pub(crate) event_selectors: Vec<devo_protocol::native::event::StreamSelector>,
    pub(crate) outbound_tx: mpsc::Sender<OutboundFrame>,
    pub(crate) opt_out_notification_methods: HashSet<String>,
    pub(crate) subscriptions: Vec<SubscriptionFilter>,
    next_event_seq: u64,
    next_client_request_id: u64,
    pending_client_requests: HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>,
}

/// Returns whether `method` is a connection-local composer notification.
///
/// These events are scoped to the requesting client connection, not to a
/// durable session subscription. `command/exec/*` belongs here because a
/// command process is keyed by its owning connection and dies with it —
/// its output must reach the owner even with no session subscription and
/// can never fan out to session subscribers.
pub(super) fn is_connection_local_notification(method: &str) -> bool {
    matches!(
        method,
        "search/updated"
            | "search/completed"
            | "search/failed"
            | "command/exec/outputDelta"
            | "command/exec/exited"
    )
}

impl ConnectionRuntime {
    /// Connection-local notifications bypass session subscription filters.
    pub(super) fn should_deliver_connection_local(&self, method: &str) -> bool {
        !self.opt_out_notification_methods.contains(method)
    }

    /// Native wire frames are identity-only (`item/*`, `turn/*`, deltas).
    /// First-party IM and Desktop share this vocabulary; ACP projects separately.
    fn native_notification_frames(
        &mut self,
        wire_method: String,
        wire_params: serde_json::Value,
        _notification: &ServerNotification,
    ) -> Vec<(String, serde_json::Value)> {
        vec![(wire_method, wire_params)]
    }

    fn prepare_notification_outbound(
        &mut self,
        connection_id: u64,
        wire_method: String,
        wire_params: serde_json::Value,
        notification: &ServerNotification,
    ) -> Vec<(mpsc::Sender<OutboundFrame>, OutboundFrame)> {
        let frames = match self.protocol {
            Some(ConnectionProtocol::Native) => {
                self.native_notification_frames(wire_method, wire_params, notification)
            }
            Some(ConnectionProtocol::Acp) => {
                vec![acp_notification_from_server_notification(notification)]
            }
            None => return Vec::new(),
        };
        let outbound_tx = self.outbound_tx.clone();
        frames
            .into_iter()
            .map(|(frame_method, value)| {
                let event_seq = self.next_seq();
                (
                    outbound_tx.clone(),
                    OutboundFrame::notification(connection_id, frame_method, event_seq, value),
                )
            })
            .collect()
    }

    pub(super) fn should_deliver(
        &self,
        method: &str,
        session_id: Option<SessionId>,
        child_parent_by_session: &HashMap<SessionId, SessionId>,
    ) -> bool {
        if self.opt_out_notification_methods.contains(method) {
            return false;
        }
        if self.subscriptions.is_empty() {
            return false;
        }
        self.subscriptions.iter().any(|subscription| {
            let session_matches = subscription.session_matches(session_id, child_parent_by_session);
            let event_matches =
                subscription.event_types.is_empty() || subscription.event_types.contains(method);
            session_matches && event_matches
        })
    }

    pub(super) fn next_seq(&mut self) -> u64 {
        let seq = self.next_event_seq;
        self.next_event_seq += 1;
        seq
    }
}

pub(crate) struct SubscriptionFilter {
    pub(crate) session_id: Option<SessionId>,
    pub(crate) event_types: HashSet<String>,
    pub(crate) include_child_agents: bool,
}

impl SubscriptionFilter {
    fn session_matches(
        &self,
        session_id: Option<SessionId>,
        child_parent_by_session: &HashMap<SessionId, SessionId>,
    ) -> bool {
        let Some(expected) = self.session_id.as_ref() else {
            return true;
        };
        if session_id.as_ref() == Some(expected) {
            return true;
        }
        self.include_child_agents
            && session_id
                .as_ref()
                .and_then(|session_id| child_parent_by_session.get(session_id))
                == Some(expected)
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
