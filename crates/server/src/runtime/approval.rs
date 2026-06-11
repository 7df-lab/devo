use super::*;

enum PolicyAuthorization {
    Allow,
    Ask,
}

enum AutoReviewOutcome {
    Approve,
    Deny(String),
    AskUser,
}

impl ServerRuntime {
    pub(super) async fn handle_approval_respond(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ApprovalRespondParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid approval/respond params: {error}"),
                );
            }
        };

        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let approval_id = params.approval_id.to_string();
        let pending = {
            let mut session = session_arc.lock().await;
            let Some(pending) = session.pending_approvals.remove(&approval_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::ApprovalNotFound,
                    "no pending approval request exists for this runtime",
                );
            };
            if pending.turn_id != params.turn_id {
                session.pending_approvals.insert(approval_id, pending);
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "approval request belongs to a different turn",
                );
            }

            if matches!(params.decision, ApprovalDecisionValue::Approve) {
                apply_approval_scope(&mut session, &params.scope, &pending);
            }
            pending
        };

        self.emit_turn_item(
            params.session_id,
            params.turn_id,
            ItemKind::ApprovalDecision,
            TurnItem::ApprovalDecision(ApprovalDecisionItem {
                approval_id: approval_id.clone(),
                decision: approval_decision_label(&params.decision).to_string(),
                scope: approval_scope_label(&params.scope).to_string(),
            }),
            serde_json::to_value(devo_protocol::ApprovalDecisionPayload {
                approval_id: approval_id.clone().into(),
                decision: approval_decision_label(&params.decision).to_string(),
                scope: approval_scope_label(&params.scope).to_string(),
            })
            .expect("serialize approval decision payload"),
        )
        .await;

        let _ = pending.tx.send(params.decision);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "approval_id": approval_id }),
        })
        .expect("serialize approval response")
    }

    pub(super) fn build_permission_checker(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        permission_mode: PermissionMode,
        permission_profile: devo_safety::RuntimePermissionProfile,
    ) -> PermissionChecker {
        let runtime = Arc::clone(self);
        PermissionChecker::new(move |request| {
            let runtime = Arc::clone(&runtime);
            let permission_profile = permission_profile.clone();
            Box::pin(async move {
                runtime
                    .authorize_tool_request(
                        session_id,
                        turn_id,
                        permission_mode,
                        permission_profile,
                        request,
                    )
                    .await
            })
        })
    }

    async fn authorize_tool_request(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        permission_mode: PermissionMode,
        permission_profile: devo_safety::RuntimePermissionProfile,
        request: ToolPermissionRequest,
    ) -> Result<(), String> {
        if let Some(result) = permission_mode_authorization(permission_mode) {
            if let Err(reason) = result {
                self.run_permission_denied_hook(session_id, &request, &reason)
                    .await;
                return Err(reason);
            }
            return Ok(());
        }
        if self.approval_cache_allows(session_id, &request).await {
            return Ok(());
        }
        match self.policy_decision(&permission_profile, &request) {
            PolicyAuthorization::Allow => Ok(()),
            PolicyAuthorization::Ask => {
                if let Some(reason) = self
                    .permission_request_hook_block_reason(session_id, &request)
                    .await
                {
                    let message = format!("blocked by PermissionRequest hook: {reason}");
                    self.run_permission_denied_hook(session_id, &request, &message)
                        .await;
                    return Err(message);
                }
                if matches!(
                    permission_profile.reviewer,
                    devo_safety::ApprovalsReviewer::AutoReview
                ) {
                    match self
                        .auto_review_tool_request(session_id, turn_id, &request)
                        .await
                    {
                        AutoReviewOutcome::Approve => return Ok(()),
                        AutoReviewOutcome::Deny(reason) => {
                            self.run_permission_denied_hook(session_id, &request, &reason)
                                .await;
                            return Err(format!("rejected by auto-reviewer: {reason}"));
                        }
                        AutoReviewOutcome::AskUser => {}
                    }
                }
                let result = self
                    .request_tool_approval(session_id, turn_id, request.clone())
                    .await;
                if let Err(reason) = &result {
                    self.run_permission_denied_hook(session_id, &request, reason)
                        .await;
                }
                result
            }
        }
    }

    async fn permission_request_hook_block_reason(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<String> {
        let report = self
            .run_session_hook(
                session_id,
                devo_core::HookEvent::PermissionRequest,
                permission_tool_extra(request),
            )
            .await;
        report.first_blocking_reason().map(str::to_string)
    }

    async fn run_permission_denied_hook(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
        reason: &str,
    ) {
        let mut extra = permission_tool_extra(request);
        extra.insert(
            "tool_use_id".to_string(),
            serde_json::json!(request.tool_call_id),
        );
        extra.insert("reason".to_string(), serde_json::json!(reason));
        self.run_session_hook(session_id, devo_core::HookEvent::PermissionDenied, extra)
            .await;
    }

    async fn auto_review_tool_request(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
    ) -> AutoReviewOutcome {
        let model = {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return AutoReviewOutcome::AskUser;
            };
            let session = session_arc.lock().await;
            session
                .summary
                .model
                .clone()
                .unwrap_or_else(|| self.deps.default_model.clone())
        };
        let response = match self
            .deps
            .provider
            .completion(build_approval_review_request(model, request))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    error = %error,
                    "auto-review approval request failed"
                );
                return AutoReviewOutcome::AskUser;
            }
        };
        match parse_reviewer_decision(&response.content) {
            Some(ReviewerDecision::Approve { rationale }) => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review approved tool request"
                );
                self.emit_auto_review_decision(
                    session_id,
                    turn_id,
                    request,
                    "approve",
                    rationale.as_str(),
                )
                .await;
                AutoReviewOutcome::Approve
            }
            Some(ReviewerDecision::Deny { rationale }) => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review denied tool request"
                );
                self.emit_auto_review_decision(
                    session_id,
                    turn_id,
                    request,
                    "deny",
                    rationale.as_str(),
                )
                .await;
                AutoReviewOutcome::Deny(rationale)
            }
            Some(ReviewerDecision::Uncertain { rationale }) => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    rationale = %rationale,
                    "auto-review deferred tool request to user"
                );
                AutoReviewOutcome::AskUser
            }
            None => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    "auto-review returned an invalid decision"
                );
                AutoReviewOutcome::AskUser
            }
        }
    }

    async fn emit_auto_review_decision(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
        decision: &str,
        rationale: &str,
    ) {
        let approval_id = format!("auto-review-{}", request.tool_call_id);
        self.emit_turn_item(
            session_id,
            turn_id,
            ItemKind::ApprovalDecision,
            TurnItem::ApprovalDecision(ApprovalDecisionItem {
                approval_id: approval_id.clone(),
                decision: format!("auto_review_{decision}"),
                scope: "auto_review".to_string(),
            }),
            serde_json::json!({
                "approval_id": approval_id,
                "decision": format!("auto_review_{decision}"),
                "scope": "auto_review",
                "rationale": rationale,
                "tool_name": request.tool_name,
                "resource": format!("{:?}", request.resource),
                "target": request.target,
            }),
        )
        .await;
    }

    fn policy_decision(
        &self,
        profile: &devo_safety::RuntimePermissionProfile,
        request: &ToolPermissionRequest,
    ) -> PolicyAuthorization {
        if profile.auto_approve {
            return PolicyAuthorization::Allow;
        }
        if request_forces_approval(request) {
            return PolicyAuthorization::Ask;
        }
        match request.resource {
            devo_safety::ResourceKind::Network => {
                if profile.allow_network {
                    PolicyAuthorization::Allow
                } else {
                    PolicyAuthorization::Ask
                }
            }
            devo_safety::ResourceKind::ShellExec => {
                if profile.allow_shell_commands {
                    PolicyAuthorization::Allow
                } else {
                    PolicyAuthorization::Ask
                }
            }
            devo_safety::ResourceKind::FileWrite => {
                let Some(path) = request.path.as_ref() else {
                    return PolicyAuthorization::Ask;
                };
                if profile
                    .writable_roots
                    .iter()
                    .any(|root| path.starts_with(root))
                {
                    PolicyAuthorization::Allow
                } else {
                    PolicyAuthorization::Ask
                }
            }
            devo_safety::ResourceKind::FileRead | devo_safety::ResourceKind::Custom(_) => {
                PolicyAuthorization::Allow
            }
        }
    }

    async fn approval_cache_allows(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> bool {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return false;
        };
        let session = session_arc.lock().await;
        cache_allows(&session.session_approval_cache, request)
            || cache_allows(&session.turn_approval_cache, request)
    }

    async fn request_tool_approval(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: ToolPermissionRequest,
    ) -> Result<(), String> {
        let approval_id = format!("approval-{}", request.tool_call_id);
        let (tx, rx) = oneshot::channel();
        let available_scopes = approval_scopes_for_request(&request);

        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        {
            let mut session = session_arc.lock().await;
            session.pending_approvals.insert(
                approval_id.clone(),
                PendingApproval {
                    turn_id,
                    tool_name: request.tool_name.clone(),
                    path: request.path.clone(),
                    host: request.host.clone(),
                    command_prefix: request.command_prefix.clone(),
                    tx,
                },
            );
        }

        let request_context = crate::PendingServerRequestContext {
            request_id: approval_id.clone().into(),
            request_kind: crate::ServerRequestKind::ItemPermissionsRequestApproval,
            session_id,
            turn_id: Some(turn_id),
            item_id: None,
        };
        let justification = request
            .justification
            .clone()
            .unwrap_or_else(|| "Tool execution requires approval.".to_string());
        let payload = ApprovalRequestPayload {
            request: request_context,
            approval_id: approval_id.clone().into(),
            action_summary: request.action_summary.clone(),
            justification: justification.clone(),
            resource: Some(format!("{:?}", request.resource)),
            available_scopes: available_scopes.clone(),
            path: request.path.as_ref().map(|path| path.display().to_string()),
            host: request.host.clone(),
            target: request.target.clone(),
        };
        self.emit_turn_item(
            session_id,
            turn_id,
            ItemKind::ApprovalRequest,
            TurnItem::ApprovalRequest(ApprovalRequestItem {
                approval_id: approval_id.clone(),
                action_summary: request.action_summary,
                justification,
                resource: Some(format!("{:?}", request.resource)),
                available_scopes,
                path: request.path.map(|path| path.display().to_string()),
                host: request.host,
                target: request.target,
            }),
            serde_json::to_value(payload).expect("serialize approval request payload"),
        )
        .await;

        match rx.await {
            Ok(ApprovalDecisionValue::Approve) => Ok(()),
            Ok(ApprovalDecisionValue::Deny) => Err("rejected by user".to_string()),
            Ok(ApprovalDecisionValue::Cancel) => Err("cancelled by user".to_string()),
            Err(_) => Err("approval channel closed".to_string()),
        }
    }
}

fn approval_decision_label(decision: &ApprovalDecisionValue) -> &'static str {
    match decision {
        ApprovalDecisionValue::Approve => "approve",
        ApprovalDecisionValue::Deny => "deny",
        ApprovalDecisionValue::Cancel => "cancel",
    }
}

fn approval_scope_label(scope: &ApprovalScopeValue) -> &'static str {
    match scope {
        ApprovalScopeValue::Once => "once",
        ApprovalScopeValue::Turn => "turn",
        ApprovalScopeValue::Session => "session",
        ApprovalScopeValue::PathPrefix => "path_prefix",
        ApprovalScopeValue::Host => "host",
        ApprovalScopeValue::Tool => "tool",
        ApprovalScopeValue::CommandPrefix => "command_prefix",
    }
}

fn approval_scopes_for_request(request: &ToolPermissionRequest) -> Vec<String> {
    let mut scopes = vec![
        "once".to_string(),
        "turn".to_string(),
        "session".to_string(),
    ];
    if request.path.is_some() {
        scopes.push("path_prefix".to_string());
    }
    if request.host.is_some() {
        scopes.push("host".to_string());
    }
    if request.command_prefix.is_some() {
        scopes.push("command_prefix".to_string());
    }
    scopes.push("tool".to_string());
    scopes
}

fn apply_approval_scope(
    session: &mut RuntimeSession,
    scope: &ApprovalScopeValue,
    pending: &PendingApproval,
) {
    match scope {
        ApprovalScopeValue::Once => {}
        ApprovalScopeValue::Turn => {
            session
                .turn_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::Session => {
            session
                .session_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::PathPrefix => {
            if let Some(path) = pending.path.clone() {
                session.turn_approval_cache.path_prefixes.insert(path);
            }
        }
        ApprovalScopeValue::Host => {
            if let Some(host) = pending.host.clone() {
                session.turn_approval_cache.hosts.insert(host);
            }
        }
        ApprovalScopeValue::Tool => {
            session
                .turn_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::CommandPrefix => {
            if let Some(command_prefix) = pending.command_prefix.clone() {
                session
                    .session_approval_cache
                    .command_prefixes
                    .insert(command_prefix);
            }
        }
    }
}

fn cache_allows(
    cache: &crate::execution::ApprovalGrantCache,
    request: &ToolPermissionRequest,
) -> bool {
    if cache.tools.contains(&request.tool_name) {
        return true;
    }
    if request
        .host
        .as_ref()
        .is_some_and(|host| cache.hosts.contains(host))
    {
        return true;
    }
    request.path.as_ref().is_some_and(|path| {
        cache
            .path_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }) || request.command_prefix.as_ref().is_some_and(|command| {
        cache
            .command_prefixes
            .iter()
            .any(|prefix| command.starts_with(prefix))
    })
}

fn request_forces_approval(request: &ToolPermissionRequest) -> bool {
    request.requests_escalation
}

fn permission_tool_extra(
    request: &ToolPermissionRequest,
) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([
        (
            "tool_name".to_string(),
            serde_json::Value::String(request.tool_name.clone()),
        ),
        ("tool_input".to_string(), request.input.clone()),
        (
            "tool_use_id".to_string(),
            serde_json::Value::String(request.tool_call_id.clone()),
        ),
    ])
}

fn permission_mode_authorization(mode: PermissionMode) -> Option<Result<(), String>> {
    match mode {
        PermissionMode::AutoApprove => Some(Ok(())),
        PermissionMode::Deny => Some(Err("approval policy is deny".to_string())),
        PermissionMode::Interactive => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_policy_strings_map_to_permission_modes() {
        assert_eq!(
            permission_mode_from_approval_policy("on-request"),
            Some(PermissionMode::Interactive)
        );
        assert_eq!(
            permission_mode_from_approval_policy("never"),
            Some(PermissionMode::AutoApprove)
        );
        assert_eq!(
            permission_mode_from_approval_policy("deny"),
            Some(PermissionMode::Deny)
        );
        assert_eq!(permission_mode_from_approval_policy("unknown"), None);
    }

    #[test]
    fn command_prefix_cache_allows_matching_command_prefix() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        cache
            .command_prefixes
            .insert(vec!["git".to_string(), "add".to_string()]);
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(cache_allows(&cache, &request));
    }

    #[test]
    fn approval_scopes_include_command_prefix_for_shell_commands() {
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(
            approval_scopes_for_request(&request)
                .iter()
                .any(|scope| scope == "command_prefix")
        );
    }

    #[test]
    fn explicit_escalation_forces_approval() {
        let mut request = test_permission_request("exec_command");
        request.requests_escalation = true;

        assert!(request_forces_approval(&request));
    }

    #[test]
    fn permission_mode_overrides_authorization_policy() {
        assert_eq!(
            permission_mode_authorization(PermissionMode::AutoApprove),
            Some(Ok(()))
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Deny),
            Some(Err("approval policy is deny".to_string()))
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Interactive),
            None
        );
    }

    fn test_permission_request(tool_name: &str) -> ToolPermissionRequest {
        ToolPermissionRequest {
            tool_call_id: "call".into(),
            tool_name: tool_name.into(),
            input: serde_json::json!({}),
            cwd: std::path::PathBuf::new(),
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            resource: devo_safety::ResourceKind::ShellExec,
            action_summary: tool_name.into(),
            justification: None,
            path: None,
            host: None,
            target: None,
            command_prefix: None,
            requests_escalation: false,
        }
    }
}
