use super::*;

use std::path::Component;
use std::path::Path;

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
        match policy_decision(&permission_profile, &request) {
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
                    .request_tool_approval(session_id, request.clone())
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
        let (model, runtime_context) = {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return AutoReviewOutcome::AskUser;
            };
            let session = session_arc.lock().await;
            (
                session
                    .summary
                    .model
                    .clone()
                    .unwrap_or_else(|| session.runtime_context.default_model.clone()),
                Arc::clone(&session.runtime_context),
            )
        };
        let response = match runtime_context
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
        request: ToolPermissionRequest,
    ) -> Result<(), String> {
        let approval_id = format!("approval-{}", request.tool_call_id);
        let (tx, rx) = oneshot::channel();
        let available_scopes = approval_scopes_for_request(&request);
        let Some(connection_id) = self
            .active_turn_connections
            .lock()
            .await
            .get(&session_id)
            .copied()
        else {
            return Err("no ACP client connection is available for permission request".to_string());
        };

        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        {
            let mut session = session_arc.lock().await;
            session.pending_approvals.insert(
                approval_id.clone(),
                PendingApproval {
                    tool_name: request.tool_name.clone(),
                    path: request.path.clone(),
                    host: request.host.clone(),
                    command_prefix: request.command_prefix.clone(),
                    tx,
                },
            );
        }

        let request_params = acp_request_permission_params(session_id, &request, &available_scopes);
        let response = match self
            .send_request_to_connection(
                connection_id,
                devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD,
                serde_json::to_value(request_params)
                    .expect("serialize ACP permission request params"),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                session_arc
                    .lock()
                    .await
                    .pending_approvals
                    .remove(&approval_id);
                return Err(format!("permission request failed: {error}"));
            }
        };
        let response: devo_protocol::AcpRequestPermissionResponse =
            match serde_json::from_value(response) {
                Ok(response) => response,
                Err(error) => {
                    session_arc
                        .lock()
                        .await
                        .pending_approvals
                        .remove(&approval_id);
                    return Err(format!(
                        "invalid session/request_permission response: {error}"
                    ));
                }
            };
        let (decision, scope) = match approval_decision_from_acp_outcome(response.outcome) {
            Ok(decision) => decision,
            Err(error) => {
                session_arc
                    .lock()
                    .await
                    .pending_approvals
                    .remove(&approval_id);
                return Err(error);
            }
        };
        let pending = {
            let mut session = session_arc.lock().await;
            let Some(pending) = session.pending_approvals.remove(&approval_id) else {
                return Err("approval request was already resolved".to_string());
            };
            if matches!(decision, ApprovalDecisionValue::Approve) {
                apply_approval_scope(&mut session, &scope, &pending);
            }
            pending
        };
        let _ = pending.tx.send(decision);

        match rx.await {
            Ok(ApprovalDecisionValue::Approve) => Ok(()),
            Ok(ApprovalDecisionValue::Deny) => Err("rejected by user".to_string()),
            Ok(ApprovalDecisionValue::Cancel) => Err("cancelled by user".to_string()),
            Err(_) => Err("approval channel closed".to_string()),
        }
    }
}

fn policy_decision(
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
        devo_safety::ResourceKind::FileRead => {
            let Some(path) = request.path.as_ref() else {
                return PolicyAuthorization::Ask;
            };
            if path_matches_any_prefix(path, &profile.readable_roots)
                || path_matches_any_prefix(path, &profile.writable_roots)
            {
                PolicyAuthorization::Allow
            } else {
                PolicyAuthorization::Ask
            }
        }
        devo_safety::ResourceKind::FileWrite => {
            let Some(path) = request.path.as_ref() else {
                return PolicyAuthorization::Ask;
            };
            if path_matches_any_prefix(path, &profile.writable_roots) {
                PolicyAuthorization::Allow
            } else {
                PolicyAuthorization::Ask
            }
        }
        devo_safety::ResourceKind::Custom(_) => PolicyAuthorization::Allow,
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

fn acp_request_permission_params(
    session_id: SessionId,
    request: &ToolPermissionRequest,
    available_scopes: &[String],
) -> devo_protocol::AcpRequestPermissionParams {
    devo_protocol::AcpRequestPermissionParams {
        session_id,
        tool_call: devo_protocol::AcpToolCallUpdate {
            tool_call_id: request.tool_call_id.clone(),
            title: Some(request.action_summary.clone()),
            kind: Some(acp_tool_kind_for_permission_request(request)),
            status: Some(devo_protocol::AcpToolCallStatus::Pending),
            raw_input: Some(request.input.clone()),
            raw_output: None,
            content: Vec::new(),
            locations: request
                .path
                .as_ref()
                .map(|path| {
                    vec![devo_protocol::AcpToolCallLocation {
                        path: path.clone(),
                        line: None,
                    }]
                })
                .unwrap_or_default(),
            meta: None,
        },
        options: acp_permission_options_for_scopes(available_scopes),
        meta: None,
    }
}

fn acp_permission_options_for_scopes(scopes: &[String]) -> Vec<devo_protocol::AcpPermissionOption> {
    let mut options = vec![devo_protocol::AcpPermissionOption {
        option_id: "allow_once".to_string(),
        name: "Allow once".to_string(),
        kind: devo_protocol::AcpPermissionOptionKind::AllowOnce,
        meta: None,
    }];
    if scopes.iter().any(|scope| scope == "session") {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_session".to_string(),
            name: "Allow for session".to_string(),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    options.push(devo_protocol::AcpPermissionOption {
        option_id: "reject_once".to_string(),
        name: "Reject".to_string(),
        kind: devo_protocol::AcpPermissionOptionKind::RejectOnce,
        meta: None,
    });
    options
}

fn acp_tool_kind_for_permission_request(
    request: &ToolPermissionRequest,
) -> devo_protocol::AcpToolKind {
    match request.resource {
        devo_safety::ResourceKind::FileRead => devo_protocol::AcpToolKind::Read,
        devo_safety::ResourceKind::FileWrite => devo_protocol::AcpToolKind::Edit,
        devo_safety::ResourceKind::ShellExec => devo_protocol::AcpToolKind::Execute,
        devo_safety::ResourceKind::Network => devo_protocol::AcpToolKind::Fetch,
        devo_safety::ResourceKind::Custom(_) => devo_protocol::AcpToolKind::Other,
    }
}

fn approval_decision_from_acp_outcome(
    outcome: devo_protocol::AcpPermissionOutcome,
) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
    match outcome {
        devo_protocol::AcpPermissionOutcome::Selected { option_id } => match option_id.as_str() {
            "allow_once" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Once)),
            "allow_session" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Session)),
            "reject_once" => Ok((ApprovalDecisionValue::Deny, ApprovalScopeValue::Once)),
            _ => Err(format!("unknown permission option selected: {option_id}")),
        },
        devo_protocol::AcpPermissionOutcome::Cancelled => {
            Ok((ApprovalDecisionValue::Cancel, ApprovalScopeValue::Once))
        }
    }
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
    request
        .path
        .as_ref()
        .is_some_and(|path| path_matches_any_prefix(path, &cache.path_prefixes))
        || request.command_prefix.as_ref().is_some_and(|command| {
            cache
                .command_prefixes
                .iter()
                .any(|prefix| command.starts_with(prefix))
        })
}

fn request_forces_approval(request: &ToolPermissionRequest) -> bool {
    request.requests_escalation
}

fn path_matches_any_prefix<'a, I>(path: &Path, prefixes: I) -> bool
where
    I: IntoIterator<Item = &'a PathBuf>,
{
    let path = normalize_permission_path(path);
    prefixes
        .into_iter()
        .any(|prefix| path.starts_with(normalize_permission_path(prefix)))
}

fn normalize_permission_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
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

    use pretty_assertions::assert_eq;

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

    #[test]
    fn path_prefix_match_normalizes_parent_components() {
        let root = abs_path(&["workspace"]);
        let inside = root.join("src").join("..").join("Cargo.toml");
        let outside = root.join("src").join("..").join("..").join("outside.txt");

        assert!(path_matches_any_prefix(&inside, [&root]));
        assert!(!path_matches_any_prefix(&outside, [&root]));
    }

    #[test]
    fn approval_path_cache_does_not_allow_parent_escape() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let root = abs_path(&["workspace", "generated"]);
        cache.path_prefixes.insert(root.clone());

        let mut escaped = test_permission_request("write");
        escaped.resource = devo_safety::ResourceKind::FileWrite;
        escaped.path = Some(root.join("..").join("outside.txt"));

        let mut allowed = test_permission_request("write");
        allowed.resource = devo_safety::ResourceKind::FileWrite;
        allowed.path = Some(root.join("..").join("generated").join("file.txt"));

        assert!(!cache_allows(&cache, &escaped));
        assert!(cache_allows(&cache, &allowed));
    }

    #[test]
    fn policy_allows_file_read_inside_readable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::ReadOnly,
            root.clone(),
        );
        let mut request = test_permission_request("read");
        request.resource = devo_safety::ResourceKind::FileRead;
        request.path = Some(root.join("Cargo.toml"));

        assert!(matches!(
            policy_decision(&profile, &request),
            PolicyAuthorization::Allow
        ));
    }

    #[test]
    fn policy_asks_for_file_read_outside_readable_roots() {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::ReadOnly,
            root,
        );
        let mut request = test_permission_request("read");
        request.resource = devo_safety::ResourceKind::FileRead;
        request.path = Some(abs_path(&["outside", "secret.txt"]));

        assert!(matches!(
            policy_decision(&profile, &request),
            PolicyAuthorization::Ask
        ));
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

    fn abs_path(parts: &[&str]) -> PathBuf {
        #[cfg(windows)]
        let mut path = PathBuf::from(r"C:\");
        #[cfg(unix)]
        let mut path = PathBuf::from("/");

        for part in parts {
            path.push(part);
        }
        path
    }
}
