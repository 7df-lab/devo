use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::ACP_AUTHENTICATE_METHOD;
use crate::ACP_INITIALIZE_METHOD;
use crate::ACP_LOGOUT_METHOD;
use crate::ACP_SESSION_CANCEL_METHOD;
use crate::ACP_SESSION_CLOSE_METHOD;
use crate::ACP_SESSION_DELETE_METHOD;
use crate::ACP_SESSION_LIST_METHOD;
use crate::ACP_SESSION_LOAD_METHOD;
use crate::ACP_SESSION_NEW_METHOD;
use crate::ACP_SESSION_PROMPT_METHOD;
use crate::ACP_SESSION_RESUME_METHOD;
use crate::ACP_SESSION_SET_CONFIG_OPTION_METHOD;
use crate::ACP_SESSION_SET_MODE_METHOD;
use crate::acp_auth_required_response;
use crate::acp_notification_from_server_event;
use crate::devo_extension_inner_method;

pub(crate) const CONNECTION_NOTIFICATION_CHANNEL_CAPACITY: usize = 4096;

const CONNECTION_NOTIFICATION_BACKPRESSURE_LOG_THRESHOLD: Duration = Duration::from_millis(50);

struct PendingConnectionNotification {
    connection_id: u64,
    kind: PendingConnectionMessageKind,
    method: String,
    event_seq: u64,
    sender: mpsc::Sender<serde_json::Value>,
    value: serde_json::Value,
}

#[derive(Clone, Copy)]
enum PendingConnectionMessageKind {
    Notification,
    JsonRpcResponse,
    ClientRequest,
}

impl ServerRuntime {
    pub async fn register_connection(
        self: &Arc<Self>,
        transport: ClientTransportKind,
        sender: mpsc::Sender<serde_json::Value>,
    ) -> u64 {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::SeqCst);
        let mut connections = self.connections.lock().await;
        connections.insert(
            connection_id,
            ConnectionRuntime {
                transport,
                state: ConnectionState::Connected,
                acp_authenticated: false,
                acp_client_capabilities: crate::AcpClientCapabilities::default(),
                sender,
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
        self.active_turn_connections
            .lock()
            .await
            .retain(|_, active_connection_id| *active_connection_id != connection_id);
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

    pub async fn handle_incoming(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) -> Option<serde_json::Value> {
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
            return Some(self.handle_acp_initialize(connection_id, id, params).await);
        }
        // Before connection enter `Ready` state, only allowed method: "initialize"
        if !self.connection_ready(connection_id).await {
            return id.map(|request_id| {
                self.error_response(
                    request_id,
                    ProtocolErrorCode::NotInitialized,
                    "connection has not completed initialize",
                )
            });
        }

        if method == ACP_AUTHENTICATE_METHOD {
            return Some(
                self.handle_acp_authenticate(connection_id, id, params)
                    .await,
            );
        }
        if method == ACP_LOGOUT_METHOD {
            return Some(self.handle_acp_logout(connection_id, id, params).await);
        }

        if !self.connection_authenticated(connection_id).await {
            if let Some(request_id) = id {
                return Some(acp_auth_required_response(request_id));
            }
            tracing::warn!(
                connection_id,
                method,
                "dropping unauthenticated client notification"
            );
            return None;
        }

        if method == ACP_SESSION_CANCEL_METHOD {
            self.handle_acp_session_cancel(params).await;
            return None;
        }

        let client_method = devo_extension_inner_method(&method).and_then(ClientMethod::parse);
        let response = match client_method {
            None if method == "session/start" => {
                let request_id = id?;
                let params: SessionStartParams = match serde_json::from_value(params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Some(self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid session/start params: {error}"),
                        ));
                    }
                };
                let response = self
                    .start_session_with_registry(connection_id, request_id, params, None)
                    .await;
                if let Ok(success) =
                    serde_json::from_value::<SuccessResponse<SessionStartResult>>(response.clone())
                {
                    self.subscribe_connection_to_session(
                        connection_id,
                        success.result.session.session_id,
                        None,
                    )
                    .await;
                }
                Some(response)
            }
            None if method == ACP_SESSION_LIST_METHOD => {
                Some(self.handle_acp_session_list(id?, params).await)
            }
            None if method == ACP_SESSION_LOAD_METHOD => Some(
                self.handle_acp_session_load(connection_id, id?, params)
                    .await,
            ),
            None if method == ACP_SESSION_NEW_METHOD => Some(
                self.handle_acp_session_new(connection_id, id?, params)
                    .await,
            ),
            None if method == ACP_SESSION_PROMPT_METHOD => {
                self.handle_acp_session_prompt(connection_id, id?, params)
                    .await
            }
            None if method == ACP_SESSION_RESUME_METHOD => Some(
                self.handle_acp_session_resume(connection_id, id?, params)
                    .await,
            ),
            None if method == ACP_SESSION_CLOSE_METHOD => {
                Some(self.handle_acp_session_close(id?, params).await)
            }
            None if method == ACP_SESSION_DELETE_METHOD => {
                Some(self.handle_acp_session_delete(id?, params).await)
            }
            None if method == ACP_SESSION_SET_MODE_METHOD => {
                Some(self.handle_acp_session_set_mode(id?, params).await)
            }
            None if method == ACP_SESSION_SET_CONFIG_OPTION_METHOD => {
                Some(self.handle_acp_session_set_config_option(id?, params).await)
            }
            // Update session metadata, including the current model and reasoning effort.
            Some(ClientMethod::SessionMetadataUpdate) => {
                Some(self.handle_session_metadata_update(id?, params).await)
            }
            // update session's permission mode, including auto-approve, default, full-access, readonly
            Some(ClientMethod::SessionPermissionsUpdate) => {
                Some(self.handle_session_permissions_update(id?, params).await)
            }
            // update session title, user may customized session title from ui client
            Some(ClientMethod::SessionTitleUpdate) => {
                Some(self.handle_session_title_update(id?, params).await)
            }
            // resume a history session, server load the jsonl file then replay the events in jsonl
            Some(ClientMethod::SessionResume) => {
                Some(self.handle_session_resume(connection_id, id?, params).await)
            }
            // fork a given session at given user turn index
            Some(ClientMethod::SessionFork) => {
                Some(self.handle_session_fork(connection_id, id?, params).await)
            }
            // rollback session at given point
            Some(ClientMethod::SessionRollback) => Some(
                self.handle_session_rollback(connection_id, id?, params)
                    .await,
            ),
            // compact session context history
            Some(ClientMethod::SessionCompact) => {
                Some(self.handle_session_compact(id?, params).await)
            }
            // list the current skills, including given cwd param
            Some(ClientMethod::SkillsList) => Some(self.handle_skills_list(id?, params).await),
            // TODO: not sure what is the endpoint
            Some(ClientMethod::SkillsChanged) => {
                Some(self.handle_skills_changed(id?, params).await)
            }
            Some(ClientMethod::SkillsSetEnabled) => {
                Some(self.handle_skills_set_enabled(id?, params).await)
            }
            // get the model catalog, aka the configured models list
            Some(ClientMethod::ModelCatalog) => Some(self.handle_model_catalog(id?, params).await),
            // TODO: not sure, config model from client should be deprecated
            Some(ClientMethod::ModelSaved) => Some(self.handle_model_saved(id?, params).await),
            Some(ClientMethod::CommandExec) => {
                Some(self.handle_command_exec(connection_id, id?, params).await)
            }
            Some(ClientMethod::CommandExecWrite) => Some(
                self.handle_command_exec_write(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::CommandExecResize) => Some(
                self.handle_command_exec_resize(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::CommandExecTerminate) => Some(
                self.handle_command_exec_terminate(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::MessageEditPrevious) => {
                Some(self.handle_message_edit_previous(id?, params).await)
            }
            // TODO: start a new user turn, maybe should change name to "turn/submit"
            Some(ClientMethod::TurnStart) => Some(
                self.handle_turn_start_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            Some(ClientMethod::TurnShellCommand) => Some(
                self.handle_turn_shell_command_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            // interupt the current working turn
            Some(ClientMethod::TurnInterrupt) => {
                Some(self.handle_turn_interrupt(id?, params).await)
            }
            Some(ClientMethod::TurnSteer) => {
                Some(self.handle_turn_steer(connection_id, id?, params).await)
            }
            Some(ClientMethod::RequestUserInputRespond) => {
                Some(self.handle_request_user_input_respond(id?, params).await)
            }
            Some(ClientMethod::SearchStart) => Some(
                self.handle_reference_search_start(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::SearchUpdate) => {
                Some(self.handle_reference_search_update(id?, params).await)
            }
            Some(ClientMethod::SearchCancel) => {
                Some(self.handle_reference_search_cancel(id?, params).await)
            }
            Some(ClientMethod::EventsSubscribe) => Some(
                self.handle_events_subscribe(connection_id, id?, params)
                    .await,
            ),
            // TODO: the goal design should be simplified
            Some(ClientMethod::GoalCreate) => Some(self.handle_goal_create(id?, params).await),
            Some(ClientMethod::GoalSet) => Some(self.handle_goal_set(id?, params).await),
            Some(ClientMethod::GoalPause) => Some(self.handle_goal_pause(id?, params).await),
            Some(ClientMethod::GoalResume) => Some(self.handle_goal_resume(id?, params).await),
            Some(ClientMethod::GoalComplete) => Some(self.handle_goal_complete(id?, params).await),
            // cancel the current goal loop
            Some(ClientMethod::GoalCancel) => Some(self.handle_goal_cancel(id?, params).await),
            Some(ClientMethod::GoalClear) => Some(self.handle_goal_clear(id?, params).await),
            Some(ClientMethod::GoalStatus) => Some(self.handle_goal_status(id?, params).await),
            Some(ClientMethod::AgentSpawn) => Some(self.handle_agent_spawn(id?, params).await),
            Some(ClientMethod::AgentSendMessage) => {
                Some(self.handle_agent_send_message(id?, params).await)
            }
            Some(ClientMethod::AgentWait) => Some(self.handle_agent_wait(id?, params).await),
            // TODO: list the current sub agents, not sure whther the current agent is right.
            Some(ClientMethod::AgentList) => Some(self.handle_agent_list(id?, params).await),
            // TODO: get the agent status, it is the subagent session status, maybe the design is not right, wait for reviewing.
            Some(ClientMethod::AgentStatus) => Some(self.handle_agent_status(id?, params).await),
            Some(ClientMethod::AgentClose) => Some(self.handle_agent_close(id?, params).await),
            // TODO: list the current provider vender list
            Some(ClientMethod::ProviderVendorList) => {
                Some(self.handle_provider_vendor_list(id?, params).await)
            }
            Some(ClientMethod::ProviderValidate) => {
                Some(self.handle_provider_validate(id?, params).await)
            }
            // TODO: update / add provider vendor to the provider vendor list
            Some(ClientMethod::ProviderVendorUpsert) => {
                Some(self.handle_provider_vendor_upsert(id?, params).await)
            }
            // TODO: add endpoint to kill background process opened by unified exec command.
            // TODO: add endpoint to list current background processes.
            None => Some(self.error_response(
                id?,
                ProtocolErrorCode::InvalidParams,
                format!("unknown method: {method}"),
            )),
        };
        // Filter out responses already dispatched via the high-priority channel.
        match response {
            Some(serde_json::Value::Null) => None,
            other => other,
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

    pub(super) async fn emit_to_connection(
        &self,
        connection_id: u64,
        method: &str,
        event: ServerEvent,
    ) {
        let session_id = event.session_id();
        let child_parent_by_session = self.child_parent_by_session().await;
        let notification = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if !connection.should_deliver(method, session_id, &child_parent_by_session) {
                return;
            }
            let event_seq = connection.next_seq();
            let event = event.with_seq(event_seq);
            let (method, value) = acp_notification_from_server_event(method, &event);
            Some(PendingConnectionNotification {
                connection_id,
                kind: PendingConnectionMessageKind::Notification,
                method,
                event_seq,
                sender: connection.sender.clone(),
                value,
            })
        };
        if let Some(notification) = notification {
            send_connection_notification(notification).await;
        }
    }

    pub(super) async fn broadcast_event(&self, event: ServerEvent) {
        if let ServerEvent::TurnCompleted(payload) = &event {
            self.account_goal_turn_completed(&payload.turn).await;
        }
        self.record_subagent_output_event(&event).await;
        let method = event.method_name();
        let session_id = event.session_id();
        let child_parent_by_session = self.child_parent_by_session().await;
        let active_turn_connections = self.active_turn_connections.lock().await.clone();
        let notifications = {
            let mut connections = self.connections.lock().await;
            connections
                .iter_mut()
                .filter_map(|(connection_id, connection)| {
                    if should_skip_non_owner_stdio_stream(
                        *connection_id,
                        connection,
                        &event,
                        &active_turn_connections,
                    ) {
                        return None;
                    }
                    if !connection.should_deliver(method, session_id, &child_parent_by_session) {
                        return None;
                    }
                    let event_seq = connection.next_seq();
                    let event = event.clone().with_seq(event_seq);
                    let (method, value) = acp_notification_from_server_event(method, &event);
                    Some(PendingConnectionNotification {
                        connection_id: *connection_id,
                        kind: PendingConnectionMessageKind::Notification,
                        method,
                        event_seq,
                        sender: connection.sender.clone(),
                        value,
                    })
                })
                .collect::<Vec<_>>()
        };
        for notification in notifications {
            send_connection_notification(notification).await;
        }
    }

    pub(super) async fn send_raw_to_connection(
        &self,
        connection_id: u64,
        value: serde_json::Value,
    ) {
        let notification = {
            let connections = self.connections.lock().await;
            let Some(connection) = connections.get(&connection_id) else {
                return;
            };
            PendingConnectionNotification {
                connection_id,
                kind: PendingConnectionMessageKind::JsonRpcResponse,
                method: "<response>".to_string(),
                event_seq: 0,
                sender: connection.sender.clone(),
                value,
            }
        };
        send_connection_notification(notification).await;
    }

    pub(super) async fn send_request_to_connection(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            /*timeout_duration*/ None,
            CancellationToken::new(),
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
    ) -> Result<serde_json::Value, String> {
        let (request_id, receiver, notification) = {
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
                PendingConnectionNotification {
                    connection_id,
                    kind: PendingConnectionMessageKind::ClientRequest,
                    method: method.to_string(),
                    event_seq: 0,
                    sender: connection.sender.clone(),
                    value,
                },
            )
        };
        let mut pending_request = PendingClientRequestGuard::new(
            Arc::clone(&self.connections),
            connection_id,
            request_id,
        );
        if !send_connection_notification(notification).await {
            pending_request.remove().await;
            return Err("client connection closed before request was sent".to_string());
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
                let message = receiver
                    .await
                    .map_err(|_| "client connection closed before responding".to_string())??;
                pending_request.disarm();
                message
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

fn should_skip_non_owner_stdio_stream(
    connection_id: u64,
    connection: &ConnectionRuntime,
    event: &ServerEvent,
    active_turn_connections: &HashMap<SessionId, u64>,
) -> bool {
    if !matches!(
        connection.transport,
        ClientTransportKind::Stdio | ClientTransportKind::StdioProxy
    ) {
        return false;
    }

    let ServerEvent::ItemDelta {
        delta_kind:
            ItemDeltaKind::AgentMessageDelta
            | ItemDeltaKind::ReasoningSummaryTextDelta
            | ItemDeltaKind::ReasoningTextDelta,
        payload,
    } = event
    else {
        return false;
    };

    if payload.context.turn_id.is_none() {
        return false;
    }

    active_turn_connections
        .get(&payload.context.session_id)
        .is_some_and(|active_connection_id| *active_connection_id != connection_id)
}

pub(crate) struct ConnectionRuntime {
    pub(crate) transport: ClientTransportKind,
    pub(crate) state: ConnectionState,
    pub(crate) acp_authenticated: bool,
    pub(crate) acp_client_capabilities: crate::AcpClientCapabilities,
    pub(crate) sender: mpsc::Sender<serde_json::Value>,
    pub(crate) opt_out_notification_methods: HashSet<String>,
    pub(crate) subscriptions: Vec<SubscriptionFilter>,
    next_event_seq: u64,
    next_client_request_id: u64,
    pending_client_requests: HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>,
}

impl ConnectionRuntime {
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
        let Some(expected) = self.session_id else {
            return true;
        };
        if session_id == Some(expected) {
            return true;
        }
        self.include_child_agents
            && session_id.and_then(|session_id| child_parent_by_session.get(&session_id).copied())
                == Some(expected)
    }
}

async fn send_connection_notification(notification: PendingConnectionNotification) -> bool {
    let PendingConnectionNotification {
        connection_id,
        kind,
        method,
        event_seq,
        sender,
        value,
    } = notification;
    let notification = match kind {
        PendingConnectionMessageKind::Notification => {
            serde_json::to_value(crate::NotificationEnvelope {
                method: method.clone(),
                params: value,
            })
            .expect("serialize client notification envelope")
        }
        PendingConnectionMessageKind::JsonRpcResponse
        | PendingConnectionMessageKind::ClientRequest => value,
    };
    let item_id = notification_item_id(&notification);
    let assistant_delta = notification_assistant_delta(&method, &notification);
    let delta_len = assistant_delta.map(str::len);
    let assistant_token_text = assistant_delta.and_then(assistant_token_log_preview);
    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            connection_id,
            method = %method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            assistant_token_text,
            "sending client notification"
        );
    } else {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            connection_id,
            method = %method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            "sending client notification"
        );
    }
    let reserve_started_at = Instant::now();
    let permit = match tokio::time::timeout(
        CONNECTION_NOTIFICATION_BACKPRESSURE_LOG_THRESHOLD,
        sender.reserve(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => {
            tracing::debug!(
                connection_id,
                method = %method,
                event_seq,
                "client notification receiver dropped"
            );
            return false;
        }
        Err(_) => {
            tracing::warn!(
                connection_id,
                method = %method,
                event_seq,
                threshold_ms = CONNECTION_NOTIFICATION_BACKPRESSURE_LOG_THRESHOLD.as_millis(),
                "client notification queue applying backpressure"
            );
            match sender.reserve().await {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::debug!(
                        connection_id,
                        method = %method,
                        event_seq,
                        "client notification receiver dropped during backpressure"
                    );
                    return false;
                }
            }
        }
    };
    let waited = reserve_started_at.elapsed();
    if waited >= CONNECTION_NOTIFICATION_BACKPRESSURE_LOG_THRESHOLD {
        tracing::debug!(
            connection_id,
            method = %method,
            event_seq,
            waited_ms = waited.as_millis(),
            "client notification queue accepted message after backpressure"
        );
    }
    permit.send(notification);
    true
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn notification_item_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("params")
        .and_then(|params| params.get("context"))
        .and_then(|context| context.get("item_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn notification_assistant_delta<'a>(method: &str, value: &'a serde_json::Value) -> Option<&'a str> {
    (method == "item/agentMessage/delta")
        .then(|| value.get("params")?.get("delta")?.as_str())
        .flatten()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::new();
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderVendorCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::StreamEvent;
    use devo_provider::ModelProviderSDK;
    use devo_provider::SingleProviderRouter;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("noop provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            anyhow::bail!("noop provider does not support streaming")
        }

        fn name(&self) -> &str {
            "noop-provider"
        }
    }

    fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(
            crate::db::Database::open(data_root.join("connection.db")).expect("open test database"),
        );
        ServerRuntime::new(
            data_root.to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
                "test-model".to_string(),
                Arc::new(PresetModelCatalog::default()),
                Arc::new(ProviderVendorCatalog::default()),
                Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                    bundled: Some(BundledSkillsConfig { enabled: false }),
                    ..SkillsConfig::default()
                })),
                devo_core::AgentsMdConfig::default(),
                db,
                Arc::new(std::sync::Mutex::new(
                    AppConfigStore::load(data_root.to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
        )
    }

    #[test]
    fn subscription_filter_can_match_direct_child_agents() {
        let parent = SessionId::new();
        let child = SessionId::new();
        let unrelated = SessionId::new();
        let child_parent_by_session = HashMap::from([(child, parent)]);
        let subscription = SubscriptionFilter {
            session_id: Some(parent),
            event_types: HashSet::new(),
            include_child_agents: true,
        };

        assert_eq!(
            vec![true, true, false],
            vec![
                subscription.session_matches(Some(parent), &child_parent_by_session),
                subscription.session_matches(Some(child), &child_parent_by_session),
                subscription.session_matches(Some(unrelated), &child_parent_by_session),
            ]
        );
    }

    #[tokio::test]
    async fn timed_out_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (sender, mut receiver) = mpsc::channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, sender)
            .await;

        let result = runtime
            .send_request_to_connection_with_timeout(
                connection_id,
                "fs/read_text_file",
                serde_json::json!({}),
                Duration::from_millis(1),
                CancellationToken::new(),
            )
            .await;

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/read_text_file",
                "params": {},
            })
        );
        assert!(
            result
                .expect_err("request should time out")
                .contains("timed out")
        );
        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn cancelled_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (sender, mut receiver) = mpsc::channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, sender)
            .await;
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = runtime
            .send_request_to_connection_with_timeout(
                connection_id,
                "fs/write_text_file",
                serde_json::json!({}),
                Duration::from_secs(30),
                cancel_token,
            )
            .await;

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/write_text_file",
                "params": {},
            })
        );
        assert_eq!(
            result.expect_err("request should be cancelled"),
            "client request cancelled"
        );
        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn dropped_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (sender, mut receiver) = mpsc::channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, sender)
            .await;
        let runtime_for_request = Arc::clone(&runtime);

        let handle = tokio::spawn(async move {
            runtime_for_request
                .send_request_to_connection_with_timeout(
                    connection_id,
                    "fs/read_text_file",
                    serde_json::json!({}),
                    Duration::from_secs(30),
                    CancellationToken::new(),
                )
                .await
        });

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/read_text_file",
                "params": {},
            })
        );
        handle.abort();
        let join_error = handle.await.expect_err("request task should be aborted");
        assert!(join_error.is_cancelled());

        for _ in 0..10 {
            let connections = runtime.connections.lock().await;
            let connection = connections.get(&connection_id).expect("connection");
            if connection.pending_client_requests.is_empty() {
                return Ok(());
            }
            drop(connections);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn stdio_live_agent_deltas_only_deliver_to_active_turn_owner() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let (owner_sender, mut owner_receiver) = mpsc::channel(4);
        let owner_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, owner_sender)
            .await;
        let (observer_sender, mut observer_receiver) = mpsc::channel(4);
        let observer_connection_id = runtime
            .register_connection(ClientTransportKind::StdioProxy, observer_sender)
            .await;

        runtime
            .subscribe_connection_to_session(owner_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(observer_connection_id, session_id, None)
            .await;
        runtime
            .active_turn_connections
            .lock()
            .await
            .insert(session_id, owner_connection_id);

        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::AgentMessageDelta,
                payload: ItemDeltaPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: Some(item_id),
                        seq: 0,
                    },
                    delta: "hello".to_string(),
                    stream_index: None,
                    channel: None,
                },
            })
            .await;

        let owner_message = tokio::time::timeout(Duration::from_secs(1), owner_receiver.recv())
            .await?
            .expect("owner receives live agent delta");
        assert_eq!(
            owner_message["params"]["update"],
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": "hello",
                },
                "messageId": item_id.to_string(),
            })
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), observer_receiver.recv())
                .await
                .is_err(),
            "non-owner stdio proxy connection must not receive live agent delta"
        );

        Ok(())
    }
}
