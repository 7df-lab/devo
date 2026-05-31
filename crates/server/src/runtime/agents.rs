use super::*;

impl ServerRuntime {
    // ── Agent Handlers ────────────────────────────────────────────────

    #[allow(dead_code)]
    pub(super) async fn handle_agent_spawn(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SpawnAgentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid agent/spawn params: {e}"),
                );
            }
        };

        let child_session_id = SessionId::new();
        let mut registries = self.agent_registries.lock().await;
        let registry = registries
            .entry(params.parent_session_id)
            .or_insert_with(AgentRegistry::new);

        let agent_path = format!("root/{}", params.agent_nickname);
        let metadata = SubagentMetadata {
            session_id: child_session_id,
            parent_session_id: params.parent_session_id,
            agent_path: agent_path.clone(),
            nickname: params.agent_nickname,
            role: params.agent_role,
            status: SubagentStatus::Spawning,
            spawned_at: chrono::Utc::now(),
            closed_at: None,
        };
        registry.register(params.parent_session_id, child_session_id, metadata);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({
                "child_session_id": child_session_id.to_string(),
                "agent_path": agent_path,
            }),
        })
        .expect("serialize agent spawn result")
    }

    pub(super) async fn handle_agent_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        #[derive(serde::Deserialize)]
        struct AgentListParams {
            session_id: SessionId,
        }

        let params: AgentListParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid agent/list params: {e}"),
                );
            }
        };

        let registries = self.agent_registries.lock().await;
        let agents: Vec<serde_json::Value> = registries
            .get(&params.session_id)
            .map(|registry| {
                registry
                    .children_of(params.session_id)
                    .iter()
                    .filter_map(|child_id| registry.get(*child_id))
                    .map(|meta| {
                        serde_json::json!({
                            "session_id": meta.session_id.to_string(),
                            "nickname": meta.nickname,
                            "role": meta.role,
                            "status": format!("{:?}", meta.status).to_lowercase(),
                            "agent_path": meta.agent_path,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "agents": agents }),
        })
        .expect("serialize agent list result")
    }

    pub(super) async fn handle_agent_status(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        #[derive(serde::Deserialize)]
        struct AgentStatusParams {
            parent_session_id: SessionId,
            child_session_id: SessionId,
        }

        let params: AgentStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid agent/status params: {e}"),
                );
            }
        };

        let registries = self.agent_registries.lock().await;
        let agent = registries
            .get(&params.parent_session_id)
            .and_then(|registry| registry.get(params.child_session_id));

        match agent {
            Some(meta) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: serde_json::json!({
                    "session_id": meta.session_id.to_string(),
                    "nickname": meta.nickname,
                    "role": meta.role,
                    "status": format!("{:?}", meta.status).to_lowercase(),
                    "agent_path": meta.agent_path,
                    "spawned_at": meta.spawned_at.to_rfc3339(),
                    "closed_at": meta.closed_at.map(|t| t.to_rfc3339()),
                }),
            })
            .expect("serialize agent status result"),
            None => self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "agent not found",
            ),
        }
    }
}
