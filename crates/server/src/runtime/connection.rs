use super::*;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

pub(crate) const CONNECTION_NOTIFICATION_CHANNEL_CAPACITY: usize = 4096;

const CONNECTION_NOTIFICATION_BACKPRESSURE_LOG_THRESHOLD: Duration = Duration::from_millis(50);

struct PendingConnectionNotification {
    connection_id: u64,
    method: String,
    event_seq: u64,
    sender: mpsc::Sender<serde_json::Value>,
    value: serde_json::Value,
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
                sender,
                opt_out_notification_methods: HashSet::new(),
                subscriptions: Vec::new(),
                next_event_seq: 1,
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
        let removed = connections.remove(&connection_id);
        drop(connections);
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

        if method == "initialized" {
            if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
                connection.state = ConnectionState::Ready;
            }
            tracing::info!(connection_id, "client completed initialized handshake");
            return None;
        }
        if method == "initialize" {
            return Some(self.handle_initialize(connection_id, id, params).await);
        }

        // Before connection enter `Ready` state, only allowed method: "initialized" or "initialize"
        if !self.connection_ready(connection_id).await {
            return id.map(|request_id| {
                self.error_response(
                    request_id,
                    ProtocolErrorCode::NotInitialized,
                    "connection has not completed initialize/initialized",
                )
            });
        }

        let response = match ClientMethod::parse(method.as_str()) {
            // start a session
            Some(ClientMethod::SessionStart) => {
                Some(self.handle_session_start(connection_id, id?, params).await)
            }
            // list sessions
            // TODO: Should add pagnation
            Some(ClientMethod::SessionList) => Some(self.handle_session_list(id?, params).await),
            // update session metadata, current including model and reason effort (thinking), the term 'thinking' should be changed to 'reasoning_effort'
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
            // TODO: start a new user turn, maybe should change name to "turn/submit"
            Some(ClientMethod::TurnStart) => Some(self.handle_turn_start(id?, params).await),
            Some(ClientMethod::TurnShellCommand) => {
                Some(self.handle_turn_shell_command(id?, params).await)
            }
            // interupt the current working turn
            Some(ClientMethod::TurnInterrupt) => {
                Some(self.handle_turn_interrupt(id?, params).await)
            }
            Some(ClientMethod::TurnSteer) => {
                Some(self.handle_turn_steer(connection_id, id?, params).await)
            }
            // client approval result
            Some(ClientMethod::ApprovalRespond) => {
                Some(self.handle_approval_respond(id?, params).await)
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
            Some(ClientMethod::GoalResume) => Some(self.handle_goal_resume(id?, params).await),
            // cancel the current goal loop
            Some(ClientMethod::GoalCancel) => Some(self.handle_goal_cancel(id?, params).await),
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
            connection.subscriptions.push(SubscriptionFilter {
                session_id: Some(session_id),
                event_types: desired,
                include_child_agents: false,
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
            Some(PendingConnectionNotification {
                connection_id,
                method: method.to_string(),
                event_seq,
                sender: connection.sender.clone(),
                value: serde_json::to_value(NotificationEnvelope {
                    method: method.to_string(),
                    params: event.with_seq(event_seq),
                })
                .expect("serialize notification"),
            })
        };
        if let Some(notification) = notification {
            send_connection_notification(notification).await;
        }
    }

    pub(super) async fn broadcast_event(&self, event: ServerEvent) {
        self.record_subagent_output_event(&event).await;
        let method = event.method_name();
        let session_id = event.session_id();
        let child_parent_by_session = self.child_parent_by_session().await;
        let notifications = {
            let mut connections = self.connections.lock().await;
            connections
                .iter_mut()
                .filter_map(|(connection_id, connection)| {
                    if !connection.should_deliver(method, session_id, &child_parent_by_session) {
                        return None;
                    }
                    let event_seq = connection.next_seq();
                    Some(PendingConnectionNotification {
                        connection_id: *connection_id,
                        method: method.to_string(),
                        event_seq,
                        sender: connection.sender.clone(),
                        value: serde_json::to_value(NotificationEnvelope {
                            method: method.to_string(),
                            params: event.clone().with_seq(event_seq),
                        })
                        .expect("serialize notification"),
                    })
                })
                .collect::<Vec<_>>()
        };
        for notification in notifications {
            send_connection_notification(notification).await;
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

pub(crate) struct ConnectionRuntime {
    pub(crate) transport: ClientTransportKind,
    pub(crate) state: ConnectionState,
    pub(crate) sender: mpsc::Sender<serde_json::Value>,
    pub(crate) opt_out_notification_methods: HashSet<String>,
    pub(crate) subscriptions: Vec<SubscriptionFilter>,
    next_event_seq: u64,
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
        if self.transport == ClientTransportKind::Stdio {
            return true;
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

async fn send_connection_notification(notification: PendingConnectionNotification) {
    let PendingConnectionNotification {
        connection_id,
        method,
        event_seq,
        sender,
        value,
    } = notification;
    let item_id = notification_item_id(&value);
    let assistant_delta = notification_assistant_delta(&method, &value);
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
            return;
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
                    return;
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
    permit.send(value);
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

    use pretty_assertions::assert_eq;

    use super::*;

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
}
