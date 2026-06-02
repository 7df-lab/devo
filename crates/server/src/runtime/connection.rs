use super::*;

impl ServerRuntime {
    pub async fn register_connection(
        self: &Arc<Self>,
        transport: ClientTransportKind,
        sender: mpsc::UnboundedSender<serde_json::Value>,
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
        tracing::info!(
            connection_id,
            transport = ?removed.as_ref().map(|connection| connection.transport.clone()),
            active_connections = connections.len(),
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
            // TODO: start a new user turn, maybe should change name to "turn/submit"
            Some(ClientMethod::TurnStart) => Some(self.handle_turn_start(id?, params).await),
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
            // TODO: list the current sub agents, not sure whther the current agent is right.
            Some(ClientMethod::AgentList) => Some(self.handle_agent_list(id?, params).await),
            // TODO: get the agent status, it is the subagent session status, maybe the design is not right, wait for reviewing.
            Some(ClientMethod::AgentStatus) => Some(self.handle_agent_status(id?, params).await),
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
        let mut connections = self.connections.lock().await;
        if let Some(connection) = connections.get_mut(&connection_id) {
            if !connection.should_deliver(method, session_id) {
                return;
            }
            let value = serde_json::to_value(NotificationEnvelope {
                method: method.to_string(),
                params: event.with_seq(connection.next_seq()),
            })
            .expect("serialize notification");
            let _ = connection.sender.send(value);
        }
    }

    pub(super) async fn broadcast_event(&self, event: ServerEvent) {
        let method = event.method_name();
        let session_id = event.session_id();
        let mut connections = self.connections.lock().await;
        for connection in connections.values_mut() {
            if !connection.should_deliver(method, session_id) {
                continue;
            }
            let value = serde_json::to_value(NotificationEnvelope {
                method: method.to_string(),
                params: event.clone().with_seq(connection.next_seq()),
            })
            .expect("serialize notification");
            let _ = connection.sender.send(value);
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

pub(crate) struct ConnectionRuntime {
    pub(crate) transport: ClientTransportKind,
    pub(crate) state: ConnectionState,
    pub(crate) sender: mpsc::UnboundedSender<serde_json::Value>,
    pub(crate) opt_out_notification_methods: HashSet<String>,
    pub(crate) subscriptions: Vec<SubscriptionFilter>,
    next_event_seq: u64,
}

impl ConnectionRuntime {
    pub(super) fn should_deliver(&self, method: &str, session_id: Option<SessionId>) -> bool {
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
            let session_matches = subscription
                .session_id
                .is_none_or(|expected| session_id == Some(expected));
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
}
