use super::*;

use crate::execution::PendingApproval;
use crate::runtime::permission_decision::AuthorizationDecision;
use crate::runtime::session_actor::approval_scope::{
    apply_approval_scope_to_state, apply_path_scope_to_permission_profile,
    normalize_permission_path, path_prefix_grant_root, windows_path_prefix_unsupported,
};
use crate::runtime::session_interactive::complete_approval_wait;
use chrono::Utc;
use devo_protocol::native::item::{ApprovalDecision, Item};

use std::path::Path;

enum AutoReviewOutcome {
    Approve,
    AskUser,
}

fn admit_live_approval_scope(
    is_windows: bool,
    permission_result: Result<(ApprovalDecisionValue, ApprovalScopeValue), String>,
    resource: &devo_safety::ResourceKind,
    path: Option<&Path>,
) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
    permission_result.and_then(|(decision, scope)| {
        if let Some(reason) = windows_path_prefix_unsupported(
            is_windows,
            matches!(decision, ApprovalDecisionValue::Approve),
            &scope,
            resource,
            path,
        ) {
            Err(reason.to_string())
        } else {
            Ok((decision, scope))
        }
    })
}

fn review_response_preview(content: &[devo_protocol::ResponseContent]) -> String {
    let raw = format!("{content:?}");
    match raw.char_indices().nth(240) {
        Some((index, _)) => format!("{}…", &raw[..index]),
        None => raw,
    }
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
    ) -> Result<PermissionGrant, String> {
        // Prefer the live turn-inline mode so a mid-turn settings override
        // applies at the next authorization (L2-DES-CONV-002 Phase 3); the
        // captured turn-start value is the fallback when no turn is inline.
        let permission_mode = self
            .live_permission_mode(session_id)
            .await
            .unwrap_or(permission_mode);
        if let Some(decision) = permission_mode_authorization(permission_mode) {
            return match decision {
                AuthorizationDecision::Allow { source } => {
                    trace_permission_decision(session_id, &request, source, "allow", None);
                    let grant = escalation_permission_grant(&request);
                    if grant.bypass_sandbox
                        && let Err(reason) = self
                            .check_escalation_unsandboxed_forbidden(session_id, &request.cwd)
                            .await
                    {
                        self.run_permission_denied_hook(session_id, &request, &reason)
                            .await;
                        return Err(reason);
                    }
                    Ok(grant)
                }
                AuthorizationDecision::Deny { source, reason } => {
                    self.deny_tool_request(session_id, &request, source, reason)
                        .await
                }
                AuthorizationDecision::Ask { .. } => {
                    unreachable!("permission mode override never returns ask")
                }
            };
        }
        if request.sandbox_permissions.bypasses_sandbox()
            && let Err(reason) = self
                .check_escalation_unsandboxed_forbidden(session_id, &request.cwd)
                .await
        {
            self.run_permission_denied_hook(session_id, &request, &reason)
                .await;
            return Err(reason);
        }
        if let Some(grant) = self.approval_cache_grant(session_id, &request).await {
            trace_permission_decision(
                session_id,
                &request,
                devo_protocol::native::item::ApprovalDecisionSource::User,
                "allow",
                None,
            );
            return Ok(grant);
        }
        let permission_profile = self
            .live_permission_profile(session_id)
            .await
            .unwrap_or(permission_profile);
        let policy = policy_decision(
            &permission_profile,
            &request,
            self.user_exec_policy
                .lock()
                .expect("user exec policy lock poisoned")
                .as_ref(),
        );
        match policy {
            AuthorizationDecision::Allow { source } => {
                trace_permission_decision(session_id, &request, source, "allow", None);
                Ok(escalation_permission_grant(&request))
            }
            AuthorizationDecision::Deny { source, reason } => {
                self.deny_tool_request(session_id, &request, source, reason)
                    .await
            }
            AuthorizationDecision::Ask { source } => {
                tracing::debug!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    approval_id = %request.tool_call_id,
                    decision_source = ?source,
                    "permission policy requires an interactive decision"
                );
                if let Some(reason) = self
                    .permission_request_hook_block_reason(session_id, &request)
                    .await
                {
                    return self
                        .deny_tool_request(
                            session_id,
                            &request,
                            devo_protocol::native::item::ApprovalDecisionSource::Hook,
                            format!("blocked by PermissionRequest hook: {reason}"),
                        )
                        .await;
                }
                if matches!(
                    permission_profile.reviewer,
                    devo_safety::ApprovalsReviewer::AutoReview
                ) {
                    match self
                        .auto_review_tool_request(
                            session_id,
                            turn_id,
                            &request,
                            &permission_profile,
                        )
                        .await
                    {
                        AutoReviewOutcome::Approve => {
                            trace_permission_decision(
                                session_id,
                                &request,
                                devo_protocol::native::item::ApprovalDecisionSource::AutoReview,
                                "allow",
                                None,
                            );
                            return Ok(approved_permission_grant(&request));
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

    async fn deny_tool_request(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
        source: devo_protocol::native::item::ApprovalDecisionSource,
        reason: String,
    ) -> Result<PermissionGrant, String> {
        trace_permission_decision(session_id, request, source, "deny", Some(reason.as_str()));
        self.run_permission_denied_hook(session_id, request, &reason)
            .await;
        Err(reason)
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
        permission_profile: &devo_safety::RuntimePermissionProfile,
    ) -> AutoReviewOutcome {
        let from_inline = if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            stream.turn_inline.as_ref().map(|inline| {
                let prefix = inline
                    .last_model_request
                    .lock()
                    .ok()
                    .and_then(|guard| guard.clone());
                let fallback_model = inline
                    .summary
                    .model_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| inline.hook_context.runtime_context.default_model.clone());
                (
                    Arc::clone(&inline.hook_context.runtime_context),
                    prefix,
                    fallback_model,
                )
            })
        } else {
            None
        };
        let (runtime_context, prefix, fallback_model) = if let Some(inputs) = from_inline {
            inputs
        } else {
            let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
                return AutoReviewOutcome::AskUser;
            };
            let runtime_context = reservation.runtime_context;
            let fallback_model = reservation
                .summary
                .model_name()
                .map(str::to_string)
                .unwrap_or_else(|| runtime_context.default_model.clone());
            (runtime_context, None, fallback_model)
        };

        let context = self
            .build_approval_review_context(
                session_id,
                request,
                permission_profile,
                &runtime_context,
            )
            .await;
        let model_request = match prefix {
            Some(prefix) => extend_approval_review_request(prefix, request, &context),
            None => build_approval_review_request(fallback_model, request, &context),
        };
        let provider = self.usage_ledger.instrumented_provider(
            Arc::clone(&runtime_context.provider),
            session_id,
            Some(turn_id),
            devo_protocol::native::usage::UsagePurpose::AutoReview,
        );
        let response = match provider.completion(model_request.clone()).await {
            Ok(response) => response,
            Err(first_error) => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    error = %first_error,
                    "auto-review approval request failed; retrying once"
                );
                match provider.completion(model_request).await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            tool = %request.tool_name,
                            error = %error,
                            "auto-review approval request failed after retry"
                        );
                        return AutoReviewOutcome::AskUser;
                    }
                }
            }
        };
        match parse_reviewer_decision(&response.content) {
            Some(assessment) if assessment.risk.allows_without_user() => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    risk = assessment.risk.as_str(),
                    rationale = %assessment.rationale,
                    "auto-review allowed tool request"
                );
                self.emit_auto_review_decision(session_id, turn_id, request, &assessment)
                    .await;
                // Surface the auto-decision to the user (goal rule 7 /
                // user feedback): AutoReview approving on the user's behalf
                // must be visible, not silent. Rides the same Warning item
                // channel the fence downgrade uses (TUI live + replay).
                self.emit_native_item_completed(
                    self.native_session_turn_ids(session_id, turn_id).await.0,
                    self.native_session_turn_ids(session_id, turn_id).await.1,
                    devo_protocol::native::ids::ItemId::new(),
                    Some(
                        self.allocate_item_sequence(
                            &self.native_session_turn_ids(session_id, turn_id).await.0,
                        )
                        .await,
                    ),
                    devo_protocol::native::item::Item::Warning {
                        code: "autoReview".to_string(),
                        message: format!(
                            "Auto-review allowed `{}` (risk: {}): {}",
                            request.tool_name,
                            assessment.risk.as_str(),
                            assessment.rationale
                        ),
                        retryable: false,
                    },
                )
                .await;
                AutoReviewOutcome::Approve
            }
            Some(assessment) => {
                tracing::info!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    risk = assessment.risk.as_str(),
                    rationale = %assessment.rationale,
                    "auto-review deferred high-risk tool request to user"
                );
                AutoReviewOutcome::AskUser
            }
            None => {
                tracing::warn!(
                    session_id = %session_id,
                    tool = %request.tool_name,
                    preview = %review_response_preview(&response.content),
                    "auto-review returned an invalid risk assessment"
                );
                AutoReviewOutcome::AskUser
            }
        }
    }

    async fn build_approval_review_context(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
        permission_profile: &devo_safety::RuntimePermissionProfile,
        runtime_context: &Arc<crate::session_context::SessionRuntimeContext>,
    ) -> crate::approval_reviewer::ApprovalReviewContext {
        use crate::approval_reviewer::ApprovalReviewContext;

        let profile_summary = Some(format!(
            "preset: {:?}; writable_roots: [{}]; readable_roots: [{}]; allow_shell_commands: {}; allow_network: {}",
            permission_profile.preset,
            permission_profile
                .writable_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            permission_profile
                .readable_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            permission_profile.allow_shell_commands,
            permission_profile.allow_network,
        ));

        let agents_rules = devo_core::AgentsMdManager::new(runtime_context.agents_md.clone())
            .load(&request.cwd)
            .map(|snapshot| snapshot.rendered_instructions);

        let mut transcript_tail = Vec::new();
        let mut recent_decisions = Vec::new();

        // This runs inside the session's own turn, whose actor mailbox stays
        // blocked until the turn ends; `SessionHandle` round-trips from here
        // would deadlock. Read the snapshots registered at turn start instead.
        if let Some(snapshot) = self.active_spawn_snapshot_for_session(session_id).await {
            for item in snapshot.stable_items.iter().rev().take(10).rev() {
                transcript_tail.push(format_persisted_turn_item(item));
            }
        }
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                push_recent_approval_decisions(
                    "session",
                    &inline.session_approval_cache,
                    &mut recent_decisions,
                );
                push_recent_approval_decisions(
                    "turn",
                    &inline.turn_approval_cache,
                    &mut recent_decisions,
                );
            }
        }

        ApprovalReviewContext {
            profile_summary,
            agents_rules,
            transcript_tail,
            recent_decisions,
        }
    }

    async fn emit_auto_review_decision(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &ToolPermissionRequest,
        _assessment: &crate::approval_reviewer::ReviewerAssessment,
    ) {
        let approval_id = format!("auto-review-{}", request.tool_call_id);
        let (native_session_id, native_turn_id) =
            self.native_session_turn_ids(session_id, turn_id).await;
        let item_id = devo_protocol::native::ids::ItemId::new();
        let item_seq = self.allocate_item_sequence(&native_session_id).await;
        self.persist_completed_approval_item(
            native_session_id,
            native_turn_id,
            item_id,
            item_seq,
            &approval_id,
            request,
            devo_protocol::native::item::ApprovalDecisionKind::Approved,
            devo_protocol::native::item::ApprovalDecisionSource::AutoReview,
        )
        .await;
        self.emit_native_item_completed(
            native_session_id,
            native_turn_id,
            item_id,
            Some(item_seq),
            native_decided_approval_item(
                &approval_id,
                request,
                &[],
                native_approval_target(request),
                ApprovalDecision {
                    decision: devo_protocol::native::item::ApprovalDecisionKind::Approved,
                    scope: devo_protocol::native::item::ApprovalScope::Once,
                    decision_source:
                        devo_protocol::native::item::ApprovalDecisionSource::AutoReview,
                    decided_at: Utc::now(),
                },
            ),
        )
        .await;
    }

    async fn approval_cache_grant(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<PermissionGrant> {
        if let Some(grant) = self.session_approval_cache_grant(session_id, request).await {
            return Some(grant);
        }
        if let Some(parent_session_id) = self.parent_session_id(session_id).await {
            return self
                .session_approval_cache_grant(parent_session_id, request)
                .await;
        }
        None
    }

    /// Prefer the live turn-inline profile when a turn is in flight so mid-turn
    /// PathPrefix/Session grants are visible to `policy_decision`.
    async fn live_permission_profile(
        &self,
        session_id: SessionId,
    ) -> Option<devo_safety::RuntimePermissionProfile> {
        if let Some(profile) = self.turn_inline_permission_profile(session_id).await {
            return Some(profile);
        }
        if let Some(parent_session_id) = self.parent_session_id(session_id).await {
            return self.turn_inline_permission_profile(parent_session_id).await;
        }
        None
    }

    async fn turn_inline_permission_profile(
        &self,
        session_id: SessionId,
    ) -> Option<devo_safety::RuntimePermissionProfile> {
        let stream = self.active_stream_state(session_id).await?;
        let stream = stream.lock().await;
        stream
            .turn_inline
            .as_ref()
            .map(|inline| inline.hook_context.config.permission_profile.clone())
    }

    /// Live permission mode from the turn-inline snapshot while a turn is in
    /// flight; `None` when no turn is active (the caller falls back to the
    /// turn-start capture).
    async fn live_permission_mode(&self, session_id: SessionId) -> Option<PermissionMode> {
        let stream = self.active_stream_state(session_id).await?;
        let stream = stream.lock().await;
        stream
            .turn_inline
            .as_ref()
            .map(|inline| inline.hook_context.config.permission_mode)
    }

    pub(crate) async fn apply_approval_scope_to_turn_inline(
        &self,
        session_id: SessionId,
        scope: &ApprovalScopeValue,
        pending: &PendingApproval,
    ) {
        let Some(stream) = self.active_stream_state(session_id).await else {
            return;
        };
        let mut stream = stream.lock().await;
        let Some(inline) = stream.turn_inline.as_mut() else {
            return;
        };
        apply_approval_scope_to_state(
            &mut inline.session_approval_cache,
            &mut inline.turn_approval_cache,
            scope,
            pending,
        );
        apply_path_scope_to_permission_profile(
            &mut inline.hook_context.config.permission_profile,
            scope,
            pending,
        );
    }

    async fn session_approval_cache_grant(
        &self,
        session_id: SessionId,
        request: &ToolPermissionRequest,
    ) -> Option<PermissionGrant> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return cache_grant(&inline.session_approval_cache, request)
                    .or_else(|| cache_grant(&inline.turn_approval_cache, request));
            }
        }
        let session_handle = self.session(session_id).await?;
        let cache = session_handle.approval_cache_snapshot().await?;
        cache_grant(&cache.session_approval_cache, request)
            .or_else(|| cache_grant(&cache.turn_approval_cache, request))
    }

    async fn check_escalation_unsandboxed_forbidden(
        &self,
        session_id: SessionId,
        cwd: &Path,
    ) -> Result<(), String> {
        let Some(profile_name) = self.session_sandbox_profile(session_id, cwd).await else {
            return Ok(());
        };
        if profile_name == "off" {
            return Ok(());
        }
        if !devo_sandbox::unsandboxed_execution_allowed(Some(profile_name.as_str()), cwd) {
            return Err(
                "unsandboxed execution is forbidden when the session sandbox profile has deny-read paths configured"
                    .to_string(),
            );
        }
        Ok(())
    }

    async fn session_sandbox_profile(&self, session_id: SessionId, cwd: &Path) -> Option<String> {
        // Tool authorization runs on the session actor task during an in-flight
        // turn. Prefer the turn-inline snapshot so we never wait on the actor
        // mailbox (which would deadlock).
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.hook_context.config.sandbox_profile.clone();
            }
        }
        let session_handle = self.session(session_id).await?;
        session_handle
            .shell_exec_context(cwd.to_path_buf())
            .await
            .and_then(|context| context.sandbox_profile)
    }

    async fn parent_session_id(&self, session_id: SessionId) -> Option<SessionId> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.summary.parent_session_id();
            }
        }
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        session_handle.parent_session_id().await.and_then(|p| p)
    }

    /// Sub-agent turns route interactive approvals through the root of their
    /// parent chain so the request reaches a connection that actually
    /// subscribes to the conversation.
    ///
    /// Child sessions never have client subscribers of their own, and a
    /// direct parent only holds a live connection attribution while its own
    /// turn is running — gating on that liveness orphaned children spawned by
    /// a spawn-and-return turn (parent already idle when the child first
    /// needs an approval): the request fell back to the child's own session,
    /// matched no controller, and failed as an instant permission error. The
    /// durable restore path already keys waiting-approval lanes by the
    /// parent chain without a liveness gate; this matches it.
    pub(in crate::runtime) async fn permission_host_session_id(
        &self,
        session_id: SessionId,
    ) -> SessionId {
        let mut host = session_id;
        let mut visited = std::collections::HashSet::from([session_id]);
        while let Some(parent_session_id) = self.parent_session_id(host).await {
            if !visited.insert(parent_session_id) {
                break;
            }
            host = parent_session_id;
        }
        host
    }

    /// Rule line for a PathPrefixPersist approval (§8, P3). Test-only.
    #[cfg(test)]
    pub(crate) fn path_prefix_rule_line(root: &std::path::Path, write: bool) -> String {
        format!(
            "path_prefix allow {} {}
",
            if write { "write" } else { "read" },
            root.display()
        )
    }

    /// Rule line for a HostPersist approval (§8, P3). Test-only.
    #[cfg(test)]
    pub(crate) fn host_rule_line(host: &str) -> String {
        format!(
            "network_rule allow {host}
"
        )
    }

    /// Persist a PathPrefixPersist approval (design doc §8, P3): grant the
    /// prefix as a readable/writable root in the durable sandbox config so
    /// every future session derives it into its fence — no re-asking.
    pub(crate) async fn persist_path_prefix_rule(
        &self,
        root: &std::path::Path,
        write: bool,
    ) -> Result<(), String> {
        // Durable form: the devo home rules store, same append-only + reload
        // pattern as command prefixes. The rule engine (RuntimePermissionProfile
        // derivation) reads roots from this store at session build.
        let rules_path = crate::exec_policy_store::default_user_rules_path()
            .map_err(|error| error.to_string())?;
        let line = format!(
            "path_prefix allow {} {}
",
            if write { "write" } else { "read" },
            root.display()
        );
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            if let Some(dir) = rules_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let existing = std::fs::read_to_string(&rules_path).unwrap_or_default();
            if existing.lines().any(|l| l.trim() == line.trim()) {
                return Ok(()); // idempotent
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&rules_path)
                .map_err(|e| e.to_string())?;
            file.write_all(line.as_bytes()).map_err(|e| e.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        *self
            .user_exec_policy
            .lock()
            .expect("user exec policy lock poisoned") =
            crate::exec_policy_store::load_user_exec_policy();
        Ok(())
    }

    /// Persist a HostPersist approval (§8, P3): append a network host rule to
    /// the durable rules store.
    pub(crate) async fn persist_host_rule(&self, host: &str) -> Result<(), String> {
        let rules_path = crate::exec_policy_store::default_user_rules_path()
            .map_err(|error| error.to_string())?;
        let line = format!(
            "network_rule allow {host}
"
        );
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            if let Some(dir) = rules_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let existing = std::fs::read_to_string(&rules_path).unwrap_or_default();
            if existing.lines().any(|l| l.trim() == line.trim()) {
                return Ok(());
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&rules_path)
                .map_err(|e| e.to_string())?;
            file.write_all(line.as_bytes()).map_err(|e| e.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) async fn persist_command_prefix_rule(
        &self,
        prefix: &[String],
    ) -> Result<(), String> {
        let policy_path = crate::exec_policy_store::default_user_rules_path()
            .map_err(|error| error.to_string())?;
        let prefix = prefix.to_vec();
        tokio::task::spawn_blocking(move || {
            devo_execpolicy::blocking_append_allow_prefix_rule(&policy_path, &prefix)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        *self
            .user_exec_policy
            .lock()
            .expect("user exec policy lock poisoned") =
            crate::exec_policy_store::load_user_exec_policy();
        Ok(())
    }

    async fn request_tool_approval(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: ToolPermissionRequest,
    ) -> Result<PermissionGrant, String> {
        let host_session_id = self.permission_host_session_id(session_id).await;
        let available_scopes = approval_scopes_for_request(&request);
        let owner_connection_id = self
            .active_turns
            .active_connection_id(host_session_id)
            .await
            .or(self.active_turns.active_connection_id(session_id).await);

        if host_session_id != session_id {
            tracing::debug!(
                child_session_id = %session_id,
                parent_session_id = %host_session_id,
                tool = %request.tool_name,
                "routing sub-agent permission request through parent session"
            );
        }

        let approval_id = request.tool_call_id.clone();
        let (native_session_id, native_turn_id) =
            self.native_session_turn_ids(session_id, turn_id).await;
        let approval_item_id = devo_protocol::native::ids::ItemId::new();
        let approval_item_seq = self.allocate_item_sequence(&native_session_id).await;
        let persisted_approval = self
            .persist_waiting_approval_item(
                native_session_id,
                native_turn_id,
                approval_item_id,
                approval_item_seq,
                &request,
                &available_scopes,
            )
            .await;
        let checkpoint = self
            .persist_approval_checkpoint(host_session_id, session_id, turn_id, &request)
            .await
            .map_err(|error| format!("failed to persist approval checkpoint: {error}"))?;
        let (tx, rx) = oneshot::channel();
        let (controller_tx, mut controller_rx) = tokio::sync::mpsc::unbounded_channel();
        let pending = PendingApproval {
            owner_session_id: session_id,
            turn_id,
            tool_name: request.tool_name.clone(),
            resource: Some(request.resource.clone()),
            path: request.path.clone(),
            host: request.host.clone(),
            command_prefix: request.command_prefix.clone(),
            command_pattern: request.command_pattern.clone(),
            requests_escalation: request.sandbox_permissions.requests_escalation(),
            command: devo_core::tools::command_str_for_permission_request(&request),
            cwd: request.cwd.clone(),
            sandbox_permissions: devo_core::tools::sandbox_permission_cache_key_from_input(
                &request.input,
            ),
            persisted: persisted_approval.clone(),
            checkpoint: Some(checkpoint),
            tx,
        };
        self.session_interactive
            .register_pending_approval(
                host_session_id,
                approval_id.clone(),
                pending,
                controller_tx,
                available_scopes.clone(),
            )
            .await;

        let request_params =
            acp_request_permission_params(host_session_id, &request, &available_scopes);
        // Native reverse request (L2-DES-APP-008 DD-8): the waiting-state
        // `Item::Approval` payload is the request params; the method is
        // discriminated by resource kind.
        let native_method = match request.resource {
            devo_safety::ResourceKind::ShellExec => "approval/command/request",
            devo_safety::ResourceKind::FileWrite => "approval/fileChange/request",
            devo_safety::ResourceKind::FileRead
            | devo_safety::ResourceKind::Network
            | devo_safety::ResourceKind::Custom(_) => "approval/permission/request",
        };
        let native_target = native_approval_target(&request);
        let native_params = serde_json::to_value(native_approval_item(
            &approval_id,
            &request,
            &available_scopes,
            native_target.clone(),
            None,
        ))
        .expect("serialize native approval request params");
        let cancel_token = self
            .active_turns
            .cancel_token_for_host_or_session(host_session_id, session_id)
            .await;
        let (request_ready_tx, request_ready_rx) = oneshot::channel();
        let permission_request = async {
            tokio::select! {
                result = self.request_permission_from_controllers(
                    host_session_id,
                    owner_connection_id,
                    super::control_requests::PermissionControllerRequest {
                        acp_params: request_params,
                        native_method: native_method.to_string(),
                        native_params,
                        ready: request_ready_tx,
                    },
                    cancel_token,
                ) => result,
                result = controller_rx.recv() => result.ok_or_else(|| {
                    "permission controller recovery channel closed".to_string()
                }),
            }
        };
        let publish_waiting_item = async {
            match request_ready_rx.await {
                Ok(Ok(())) => {
                    self.emit_native_item_started(
                        native_session_id,
                        native_turn_id,
                        approval_item_id,
                        Some(approval_item_seq),
                        native_approval_item(
                            &approval_id,
                            &request,
                            &available_scopes,
                            native_target.clone(),
                            None,
                        ),
                    )
                    .await;
                    Ok(())
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err("permission request readiness channel closed".to_string()),
            }
        };
        let (permission_result, publish_result) =
            tokio::join!(permission_request, publish_waiting_item);
        // Reject unsupported Windows directory-capability scopes before the
        // generic error arm below records an approved item, sends pending.tx,
        // updates any cache/profile, or persists an always-allow rule.
        let permission_result = admit_live_approval_scope(
            cfg!(windows),
            permission_result,
            &request.resource,
            request.path.as_deref(),
        );
        let (decision, scope) = match permission_result {
            Ok(decision) => decision,
            Err(error) => {
                if let Some(persisted) = &persisted_approval {
                    self.persist_resolved_approval_item(
                        native_session_id,
                        native_turn_id,
                        &request,
                        &available_scopes,
                        devo_protocol::native::item::ApprovalDecisionKind::Cancelled,
                        devo_protocol::native::item::ApprovalScope::Once,
                        devo_protocol::native::item::ApprovalDecisionSource::ExternalPolicy,
                        persisted,
                    )
                    .await;
                }
                if publish_result.is_ok() {
                    self.emit_native_item_completed(
                        native_session_id,
                        native_turn_id,
                        approval_item_id,
                        Some(approval_item_seq),
                        native_decided_approval_item(
                            &approval_id,
                            &request,
                            &available_scopes,
                            native_target.clone(),
                            ApprovalDecision {
                                decision:
                                    devo_protocol::native::item::ApprovalDecisionKind::Cancelled,
                                scope: devo_protocol::native::item::ApprovalScope::Once,
                                decision_source:
                                    devo_protocol::native::item::ApprovalDecisionSource::ExternalPolicy,
                                decided_at: Utc::now(),
                            },
                        ),
                    )
                    .await;
                }
                self.session_interactive
                    .remove_pending_approval(host_session_id, &approval_id)
                    .await;
                return Err(format!("permission request failed: {error}"));
            }
        };
        publish_result.map_err(|error| format!("permission request failed: {error}"))?;
        let (outcome, reason) = match &decision {
            ApprovalDecisionValue::Approve => ("allow", None),
            ApprovalDecisionValue::Deny => ("deny", Some("rejected by user")),
            ApprovalDecisionValue::Cancel => ("deny", Some("cancelled by user")),
        };
        trace_permission_decision(
            session_id,
            &request,
            devo_protocol::native::item::ApprovalDecisionSource::User,
            outcome,
            reason,
        );
        let native_decision = match &decision {
            ApprovalDecisionValue::Approve => {
                devo_protocol::native::item::ApprovalDecisionKind::Approved
            }
            ApprovalDecisionValue::Deny => {
                devo_protocol::native::item::ApprovalDecisionKind::Denied
            }
            ApprovalDecisionValue::Cancel => {
                devo_protocol::native::item::ApprovalDecisionKind::Cancelled
            }
        };
        if let Some(persisted) = &persisted_approval {
            self.persist_resolved_approval_item(
                native_session_id,
                native_turn_id,
                &request,
                &available_scopes,
                native_decision,
                native_approval_scope(&scope),
                devo_protocol::native::item::ApprovalDecisionSource::User,
                persisted,
            )
            .await;
        }
        self.emit_native_item_completed(
            native_session_id,
            native_turn_id,
            approval_item_id,
            Some(approval_item_seq),
            native_decided_approval_item(
                &approval_id,
                &request,
                &available_scopes,
                native_target,
                ApprovalDecision {
                    decision: native_decision,
                    scope: native_approval_scope(&scope),
                    decision_source: devo_protocol::native::item::ApprovalDecisionSource::User,
                    decided_at: Utc::now(),
                },
            ),
        )
        .await;

        if let Some(pending) = self
            .session_interactive
            .remove_pending_approval(host_session_id, &approval_id)
            .await
        {
            let _ = pending.tx.send(decision.clone());
            if matches!(decision, ApprovalDecisionValue::Approve) {
                let (scope_tx, _) = oneshot::channel();
                let pending_for_scope = PendingApproval {
                    owner_session_id: pending.owner_session_id,
                    turn_id: pending.turn_id,
                    tool_name: pending.tool_name,
                    resource: pending.resource,
                    path: pending.path,
                    host: pending.host,
                    command_prefix: pending.command_prefix,
                    command_pattern: pending.command_pattern,
                    requests_escalation: pending.requests_escalation,
                    command: pending.command,
                    cwd: pending.cwd,
                    sandbox_permissions: pending.sandbox_permissions,
                    persisted: pending.persisted,
                    checkpoint: pending.checkpoint,
                    tx: scope_tx,
                };
                // Apply durable scope via the mailbox, and update live
                // TurnInlineState so the same turn's later tool calls see
                // PathPrefix/Session grants without waiting on MergeTurn.
                self.apply_approval_scope_to_turn_inline(
                    host_session_id,
                    &scope,
                    &pending_for_scope,
                )
                .await;
                // P3 durable-rule captures (§8) — taken before the pending
                // value moves into the actor call.
                let prefix_to_persist = (scope == ApprovalScopeValue::CommandPrefixPersist)
                    .then(|| pending_for_scope.command_prefix.clone())
                    .flatten();
                let path_to_persist = matches!(scope, ApprovalScopeValue::PathPrefixPersist)
                    .then(|| {
                        pending_for_scope.path.as_ref().map(|path| {
                            (
                                crate::runtime::session_actor::approval_scope::path_prefix_grant_root(path),
                                matches!(
                                    pending_for_scope.resource.as_ref(),
                                    Some(devo_safety::ResourceKind::FileWrite)
                                ),
                            )
                        })
                    })
                    .flatten();
                let host_to_persist = matches!(scope, ApprovalScopeValue::HostPersist)
                    .then(|| pending_for_scope.host.clone())
                    .flatten();
                if let Some(session_handle) = self.session(host_session_id).await {
                    session_handle
                        .apply_approval_scope(scope, pending_for_scope)
                        .await;
                    // P3 durable rules (§8): always-scopes persist at
                    // resolution time, mirroring CommandPrefixPersist.
                    if let Some((root, write)) = path_to_persist
                        && let Err(error) = self.persist_path_prefix_rule(&root, write).await
                    {
                        tracing::warn!(
                            session_id = %host_session_id,
                            error = %error,
                            "failed to persist path prefix rule"
                        );
                    }
                    if let Some(host) = host_to_persist
                        && let Err(error) = self.persist_host_rule(&host).await
                    {
                        tracing::warn!(
                            session_id = %host_session_id,
                            error = %error,
                            "failed to persist host rule"
                        );
                    }
                    if let Some(prefix) = prefix_to_persist
                        && let Err(error) = self.persist_command_prefix_rule(&prefix).await
                    {
                        tracing::warn!(
                            session_id = %host_session_id,
                            error = %error,
                            "failed to persist command prefix rule"
                        );
                    }
                }
            }
        }

        complete_approval_wait(rx)
            .await
            .and_then(|decision| match decision {
                ApprovalDecisionValue::Approve => Ok(approved_permission_grant(&request)),
                ApprovalDecisionValue::Deny => Err("rejected by user".to_string()),
                ApprovalDecisionValue::Cancel => Err("cancelled by user".to_string()),
            })
    }
}

fn static_policy_allow() -> AuthorizationDecision {
    AuthorizationDecision::Allow {
        source: devo_protocol::native::item::ApprovalDecisionSource::StaticPolicy,
    }
}

fn static_policy_ask() -> AuthorizationDecision {
    AuthorizationDecision::Ask {
        source: devo_protocol::native::item::ApprovalDecisionSource::StaticPolicy,
    }
}

fn static_policy_if(allow: bool) -> AuthorizationDecision {
    if allow {
        static_policy_allow()
    } else {
        static_policy_ask()
    }
}

fn policy_decision(
    profile: &devo_safety::RuntimePermissionProfile,
    request: &ToolPermissionRequest,
    exec_policy: Option<&devo_execpolicy::Policy>,
) -> AuthorizationDecision {
    if profile.yolo {
        return static_policy_allow();
    }
    if request_forces_approval(request) {
        return static_policy_ask();
    }
    match request.resource {
        devo_safety::ResourceKind::Network => static_policy_if(profile.allow_network),
        devo_safety::ResourceKind::ShellExec => {
            shell_exec_policy_decision(profile, request, exec_policy)
        }
        devo_safety::ResourceKind::FileRead => {
            let Some(path) = request.path.as_ref() else {
                return static_policy_ask();
            };
            static_policy_if(
                path_matches_any_prefix(path, &profile.readable_roots)
                    || path_matches_any_prefix(path, &profile.writable_roots),
            )
        }
        devo_safety::ResourceKind::FileWrite => {
            let Some(path) = request.path.as_ref() else {
                return static_policy_ask();
            };
            static_policy_if(path_matches_any_prefix(path, &profile.writable_roots))
        }
        devo_safety::ResourceKind::Custom(_) => static_policy_allow(),
    }
}

fn shell_exec_policy_decision(
    profile: &devo_safety::RuntimePermissionProfile,
    request: &ToolPermissionRequest,
    exec_policy: Option<&devo_execpolicy::Policy>,
) -> AuthorizationDecision {
    use devo_execpolicy::Decision;
    use devo_protocol::native::item::ApprovalDecisionSource;
    use devo_util_shell_command::is_dangerous_command::command_might_be_dangerous;

    if !profile.allow_shell_commands {
        return static_policy_ask();
    }
    let command = shell_command_for_policy(request);
    if command.is_empty() {
        return static_policy_ask();
    }
    // Fail closed on multi-line / control-separated commands even when a
    // pre-parsed argv is present (shlex would otherwise collapse newlines).
    if shell_command_has_control_separators(&command) {
        return static_policy_ask();
    }
    // Background `&` (not `&&`) splits jobs; argv-based checks miss the
    // trailing command, so fail closed like newlines.
    if command_contains_standalone_ampersand(&command) {
        return static_policy_ask();
    }
    let argv = shell_argv_for_policy(request, &command);

    if let (Some(policy), Some(argv)) = (exec_policy, argv.as_ref())
        && let Some(decision) =
            crate::exec_policy_store::exec_policy_decision_for_argv(policy, argv)
    {
        return match decision {
            Decision::Allow => AuthorizationDecision::Allow {
                source: ApprovalDecisionSource::ExecPolicy,
            },
            Decision::Forbidden => AuthorizationDecision::Deny {
                source: ApprovalDecisionSource::ExecPolicy,
                reason: "command blocked by user exec policy rules".to_string(),
            },
            Decision::Prompt => AuthorizationDecision::Ask {
                source: ApprovalDecisionSource::ExecPolicy,
            },
        };
    }

    if argv
        .as_ref()
        .is_some_and(|argv| command_might_be_dangerous(argv))
    {
        return static_policy_ask();
    }

    match devo_safety::evaluate_shell_command_for_profile(profile, &command, &request.cwd) {
        // NoMatch means the analyzer found no out-of-policy file access (or the
        // command was ambiguous without a definite file touch). Allow and run
        // under the session sandbox. Ask/Deny still require user approval.
        devo_safety::permission::PolicyDecision::NoMatch
        | devo_safety::permission::PolicyDecision::Allow => AuthorizationDecision::Allow {
            source: ApprovalDecisionSource::StaticPolicy,
        },
        devo_safety::permission::PolicyDecision::Ask
        | devo_safety::permission::PolicyDecision::Deny { .. } => AuthorizationDecision::Ask {
            source: ApprovalDecisionSource::StaticPolicy,
        },
    }
}

fn trace_permission_decision(
    session_id: SessionId,
    request: &ToolPermissionRequest,
    source: devo_protocol::native::item::ApprovalDecisionSource,
    outcome: &'static str,
    reason: Option<&str>,
) {
    tracing::info!(
        session_id = %session_id,
        tool = %request.tool_name,
        approval_id = %request.tool_call_id,
        decision_source = ?source,
        outcome,
        reason,
        "permission decision resolved"
    );
}

fn shell_command_for_policy(request: &ToolPermissionRequest) -> String {
    request
        .target
        .clone()
        .or_else(|| devo_core::tools::command_str_for_permission_request(request))
        .unwrap_or_default()
}

fn shell_argv_for_policy(request: &ToolPermissionRequest, command: &str) -> Option<Vec<String>> {
    request
        .command_argv
        .clone()
        .or_else(|| parse_safe_shell_argv(command))
}

fn shell_command_has_control_separators(command: &str) -> bool {
    command
        .as_bytes()
        .iter()
        .any(|b| matches!(b, b'\n' | b'\r' | 0x0b | 0x0c))
}

fn parse_safe_shell_argv(command: &str) -> Option<Vec<String>> {
    if shell_command_has_control_separators(command) {
        return None;
    }
    let argv = shlex::split(command)?;
    if argv.iter().any(|token| {
        token.contains(['|', ';', '>', '<', '*', '?', '$', '(', ')'])
            || token.contains("$(")
            || command.contains("&&")
            || command.contains("||")
            || command.contains("$(")
            || command.contains('`')
            || command_contains_standalone_ampersand(command)
    }) {
        return None;
    }
    if argv.first().is_some_and(|token| {
        token.split_once('=').is_some_and(|(name, value)| {
            !name.is_empty()
                && !value.is_empty()
                && name
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                && name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        })
    }) {
        return None;
    }
    Some(argv)
}

/// True when `command` has a background `&` (not `&&`).
///
/// Uses shlex tokenization so `&` inside quoted strings (e.g. URL query
/// strings) does not count. Also treats a token that *ends* with `&`
/// (`sleep 1& rm x` → `1&`) as a background operator.
fn command_contains_standalone_ampersand(command: &str) -> bool {
    shlex::split(command).is_some_and(|argv| {
        argv.iter()
            .any(|token| token_is_background_ampersand(token))
    })
}

fn token_is_background_ampersand(token: &str) -> bool {
    token != "&&" && (token == "&" || token.ends_with('&'))
}

fn approval_scope_wire_string(scope: ApprovalScopeValue) -> String {
    serde_json::to_value(native_approval_scope(&scope))
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "once".to_string())
}

pub(super) fn approval_scopes_for_request(request: &ToolPermissionRequest) -> Vec<String> {
    let mut scopes = vec![
        approval_scope_wire_string(ApprovalScopeValue::Once),
        approval_scope_wire_string(ApprovalScopeValue::Turn),
    ];
    // Shell commands get a session grant when an exact command string is
    // Prefer exact command + cwd cache when available; generalized patterns are a fallback.
    let session_available = match request.tool_name.as_str() {
        "bash" | "shell_command" | "exec_command" => {
            devo_core::tools::command_str_for_permission_request(request).is_some()
                || request.command_pattern.is_some()
        }
        _ => true,
    };
    if session_available {
        scopes.push(approval_scope_wire_string(ApprovalScopeValue::Session));
    }
    if request.path.is_some() {
        scopes.push(approval_scope_wire_string(ApprovalScopeValue::PathPrefix));
        // P3 (§8): the durable variant — persists a readable/writable root
        // rule so future sessions derive it into their fence.
        scopes.push(approval_scope_wire_string(
            ApprovalScopeValue::PathPrefixPersist,
        ));
    }
    if request.host.is_some() {
        scopes.push(approval_scope_wire_string(ApprovalScopeValue::Host));
        scopes.push(approval_scope_wire_string(ApprovalScopeValue::HostPersist));
    }
    if let Some(prefix) = request.command_prefix.as_ref() {
        scopes.push(approval_scope_wire_string(
            ApprovalScopeValue::CommandPrefix,
        ));
        if !devo_core::tools::is_banned_prefix_suggestion(prefix) {
            scopes.push(approval_scope_wire_string(
                ApprovalScopeValue::CommandPrefixPersist,
            ));
        }
    }
    scopes.push(approval_scope_wire_string(ApprovalScopeValue::Tool));
    scopes
}

pub(super) fn native_approval_target(
    request: &ToolPermissionRequest,
) -> Option<devo_protocol::native::item::ApprovalTarget> {
    if let Some(path) = &request.path {
        Some(devo_protocol::native::item::ApprovalTarget::Path { path: path.clone() })
    } else if let Some(host) = &request.host {
        Some(devo_protocol::native::item::ApprovalTarget::Host { host: host.clone() })
    } else {
        devo_core::tools::command_str_for_permission_request(request)
            .map(|command| devo_protocol::native::item::ApprovalTarget::Command { command })
    }
}

fn native_approval_item(
    approval_id: &str,
    request: &ToolPermissionRequest,
    available_scopes: &[String],
    target: Option<devo_protocol::native::item::ApprovalTarget>,
    decision: Option<ApprovalDecision>,
) -> Item {
    Item::Approval {
        approval_id: approval_id.to_string(),
        target_item_id: None,
        action_summary: request.action_summary.clone(),
        justification: request.justification.clone().unwrap_or_default(),
        resource: Some(format!("{:?}", request.resource)),
        available_scopes: available_scopes
            .iter()
            .filter_map(|scope| {
                serde_json::to_value(scope)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
            })
            .collect(),
        command_pattern: request.command_pattern.clone(),
        command_prefix: request.command_prefix.clone(),
        target,
        decision,
    }
}

pub(super) fn native_decided_approval_item(
    approval_id: &str,
    request: &ToolPermissionRequest,
    available_scopes: &[String],
    target: Option<devo_protocol::native::item::ApprovalTarget>,
    decision: ApprovalDecision,
) -> Item {
    native_approval_item(
        approval_id,
        request,
        available_scopes,
        target,
        Some(decision),
    )
}

pub(super) fn native_approval_scope(
    scope: &ApprovalScopeValue,
) -> devo_protocol::native::item::ApprovalScope {
    match scope {
        ApprovalScopeValue::Once => devo_protocol::native::item::ApprovalScope::Once,
        ApprovalScopeValue::Turn => devo_protocol::native::item::ApprovalScope::Turn,
        ApprovalScopeValue::Session => devo_protocol::native::item::ApprovalScope::Session,
        ApprovalScopeValue::PathPrefix => devo_protocol::native::item::ApprovalScope::PathPrefix,
        ApprovalScopeValue::Host => devo_protocol::native::item::ApprovalScope::Host,
        ApprovalScopeValue::Tool => devo_protocol::native::item::ApprovalScope::Tool,
        ApprovalScopeValue::CommandPrefix => {
            devo_protocol::native::item::ApprovalScope::CommandPrefix
        }
        ApprovalScopeValue::CommandPrefixPersist => {
            devo_protocol::native::item::ApprovalScope::CommandPrefixPersist
        }
        ApprovalScopeValue::PathPrefixPersist => {
            devo_protocol::native::item::ApprovalScope::PathPrefixPersist
        }
        ApprovalScopeValue::HostPersist => devo_protocol::native::item::ApprovalScope::HostPersist,
    }
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
            content: Some(Vec::new()),
            locations: request
                .path
                .as_ref()
                .map(|path| {
                    vec![devo_protocol::AcpToolCallLocation {
                        path: path.clone(),
                        line: None,
                        meta: None,
                    }]
                })
                .map(Some)
                .unwrap_or(Some(Vec::new())),
            meta: None,
        },
        options: acp_permission_options_for_scopes(
            available_scopes,
            request.command_pattern.as_deref(),
            devo_core::tools::command_str_for_permission_request(request).as_deref(),
            request.command_prefix.as_deref(),
            request.path.as_ref(),
            request.host.as_deref(),
        ),
        meta: {
            let mut meta = serde_json::Map::new();
            if let Some(pattern) = request.command_pattern.as_ref() {
                meta.insert(
                    "commandPattern".to_string(),
                    serde_json::Value::from(pattern.clone()),
                );
            }
            if let Some(prefix) = request.command_prefix.as_ref() {
                meta.insert(
                    "commandPrefix".to_string(),
                    serde_json::Value::from(prefix.clone()),
                );
            }
            if let Some(target) = devo_core::tools::command_str_for_permission_request(request) {
                meta.insert("target".to_string(), serde_json::Value::String(target));
            }
            if let Some(justification) = request.justification.as_ref() {
                meta.insert(
                    "justification".to_string(),
                    serde_json::Value::String(justification.clone()),
                );
            }
            meta.insert(
                "resource".to_string(),
                serde_json::Value::String(format!("{:?}", request.resource)),
            );
            if let Some(path) = request.path.as_ref() {
                meta.insert(
                    "path".to_string(),
                    serde_json::Value::String(path.display().to_string()),
                );
            }
            if let Some(host) = request.host.as_ref() {
                meta.insert("host".to_string(), serde_json::Value::String(host.clone()));
            }
            (!meta.is_empty()).then_some(meta)
        },
    }
}

fn acp_permission_options_for_scopes(
    scopes: &[String],
    command_pattern: Option<&[String]>,
    exact_command: Option<&str>,
    command_prefix: Option<&[String]>,
    path: Option<&std::path::PathBuf>,
    host: Option<&str>,
) -> Vec<devo_protocol::AcpPermissionOption> {
    let mut options = vec![devo_protocol::AcpPermissionOption {
        option_id: "allow_once".to_string(),
        name: "Yes, proceed".to_string(),
        kind: devo_protocol::AcpPermissionOptionKind::AllowOnce,
        meta: None,
    }];
    if scopes.iter().any(|scope| scope == "session") {
        let name = exact_command
            .map(|command| format!("Yes, and don't ask again for `{command}` in this session"))
            .or_else(|| {
                command_pattern.map(|pattern| {
                    format!(
                        "Yes, and don't ask again for `{}` in this session",
                        pattern.join(" ")
                    )
                })
            })
            .unwrap_or_else(|| {
                "Yes, and don't ask again for this command in this session".to_string()
            });
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_session".to_string(),
            name,
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes
        .iter()
        .any(|scope| scope == "commandPrefixPersist" || scope == "command_prefix_persist")
        && let Some(prefix) = command_prefix
    {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_prefix_rule".to_string(),
            name: format!(
                "Yes, and don't ask again for commands that start with `{}`",
                prefix.join(" ")
            ),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes
        .iter()
        .any(|scope| scope == "pathPrefix" || scope == "path_prefix")
        && let Some(path) = path
    {
        let root = path_prefix_grant_root(path);
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_path_prefix".to_string(),
            name: format!(
                "Yes, and don't ask again for files under `{}`",
                root.display()
            ),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes.iter().any(|scope| scope == "host")
        && let Some(host) = host
    {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_host".to_string(),
            name: format!("Yes, and allow `{host}` for this session"),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes
        .iter()
        .any(|scope| scope == "pathPrefixPersist" || scope == "path_prefix_persist")
        && let Some(path) = path
    {
        let root = path_prefix_grant_root(path);
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_path_prefix_persist".to_string(),
            name: format!(
                "Yes, and always allow files under `{}` (saved as a rule)",
                root.display()
            ),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    if scopes
        .iter()
        .any(|scope| scope == "hostPersist" || scope == "host_persist")
        && let Some(host) = host
    {
        options.push(devo_protocol::AcpPermissionOption {
            option_id: "allow_host_persist".to_string(),
            name: format!("Yes, and always allow `{host}` (saved as a rule)"),
            kind: devo_protocol::AcpPermissionOptionKind::AllowAlways,
            meta: None,
        });
    }
    options.push(devo_protocol::AcpPermissionOption {
        option_id: "reject_once".to_string(),
        name: "No, continue without running it".to_string(),
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

pub(super) fn approval_decision_from_acp_outcome(
    outcome: devo_protocol::AcpPermissionOutcome,
) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
    match outcome {
        devo_protocol::AcpPermissionOutcome::Selected { option_id } => match option_id.as_str() {
            "allow_once" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Once)),
            "allow_session" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Session)),
            "allow_prefix_rule" => Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::CommandPrefixPersist,
            )),
            "allow_path_prefix" => Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::PathPrefix,
            )),
            "allow_host" => Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Host)),
            "reject_once" => Ok((ApprovalDecisionValue::Deny, ApprovalScopeValue::Once)),
            _ => Err(format!("unknown permission option selected: {option_id}")),
        },
        devo_protocol::AcpPermissionOutcome::Cancelled => {
            Ok((ApprovalDecisionValue::Cancel, ApprovalScopeValue::Once))
        }
    }
}

fn cache_grant(
    cache: &crate::execution::ApprovalGrantCache,
    request: &ToolPermissionRequest,
) -> Option<PermissionGrant> {
    if request.sandbox_permissions.requests_escalation() {
        let key = sandbox_bypass_key_from_request(request)?;
        return cache
            .sandbox_bypass_commands
            .contains(&key)
            .then(|| PermissionGrant::from_approval(&request.sandbox_permissions));
    }
    permission_cache_matches(cache, request).then(|| {
        PermissionGrant::from_approval(&devo_core::tools::SandboxPermissionRequest::Default)
    })
}

fn permission_cache_matches(
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
    if let Some(command) = devo_core::tools::command_str_for_permission_request(request)
        && cache
            .exact_commands
            .contains(&(command, request.cwd.clone()))
    {
        return true;
    }
    if let Some(path) = request.path.as_ref() {
        let normalized = normalize_permission_path(path);
        let exact_matches = match request.resource {
            devo_safety::ResourceKind::FileWrite => cache.write_exact_paths.contains(&normalized),
            devo_safety::ResourceKind::FileRead => cache.read_exact_paths.contains(&normalized),
            _ => false,
        };
        if exact_matches {
            return true;
        }
        let prefixes = match request.resource {
            devo_safety::ResourceKind::FileWrite => &cache.write_path_prefixes,
            _ => &cache.read_path_prefixes,
        };
        if path_matches_any_prefix(path, prefixes) {
            return true;
        }
    }
    request.command_prefix.as_ref().is_some_and(|command| {
        cache
            .command_prefixes
            .iter()
            .any(|prefix| command.starts_with(prefix))
    }) || request.command_argv.as_ref().is_some_and(|argv| {
        cache
            .command_patterns
            .iter()
            .any(|pattern| devo_core::tools::command_pattern_matches(pattern, argv))
    })
}

fn request_forces_approval(request: &ToolPermissionRequest) -> bool {
    request.sandbox_permissions.requests_escalation()
}

fn escalation_permission_grant(request: &ToolPermissionRequest) -> PermissionGrant {
    PermissionGrant {
        bypass_sandbox: request.sandbox_permissions.bypasses_sandbox(),
        already_approved: false,
        sandbox_permission_overlay: request.sandbox_permissions.overlay(),
    }
}

pub(super) fn approved_permission_grant_for_request(
    request: &ToolPermissionRequest,
) -> PermissionGrant {
    PermissionGrant::from_approval(&request.sandbox_permissions)
}

fn approved_permission_grant(request: &ToolPermissionRequest) -> PermissionGrant {
    approved_permission_grant_for_request(request)
}

fn sandbox_bypass_key_from_request(
    request: &ToolPermissionRequest,
) -> Option<crate::execution::SandboxBypassKey> {
    let command = devo_core::tools::command_str_for_permission_request(request)?;
    Some(crate::execution::SandboxBypassKey {
        command,
        cwd: request.cwd.clone(),
        sandbox_permissions: devo_core::tools::sandbox_permission_cache_key_from_input(
            &request.input,
        ),
    })
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

fn permission_mode_authorization(mode: PermissionMode) -> Option<AuthorizationDecision> {
    use devo_protocol::native::item::ApprovalDecisionSource;

    match mode {
        PermissionMode::Yolo => Some(static_policy_allow()),
        PermissionMode::Deny => Some(AuthorizationDecision::Deny {
            source: ApprovalDecisionSource::StaticPolicy,
            reason: "approval policy is deny".to_string(),
        }),
        PermissionMode::Interactive => None,
    }
}

fn push_recent_approval_decisions(
    scope: &str,
    cache: &crate::execution::ApprovalGrantCache,
    recent_decisions: &mut Vec<String>,
) {
    for tool in &cache.tools {
        recent_decisions.push(format!("{scope} allow tool: {tool}"));
    }
    for host in &cache.hosts {
        recent_decisions.push(format!("{scope} allow host: {host}"));
    }
    for path in &cache.read_path_prefixes {
        recent_decisions.push(format!("{scope} allow read path: {}", path.display()));
    }
    for path in &cache.write_path_prefixes {
        recent_decisions.push(format!("{scope} allow write path: {}", path.display()));
    }
    for prefix in &cache.command_prefixes {
        recent_decisions.push(format!("{scope} allow command: {}", prefix.join(" ")));
    }
    for pattern in &cache.command_patterns {
        recent_decisions.push(format!(
            "{scope} allow command pattern: {}",
            pattern.join(" ")
        ));
    }
    for key in &cache.sandbox_bypass_commands {
        recent_decisions.push(format!(
            "{scope} allow unsandboxed command: {} ({})",
            key.command, key.sandbox_permissions
        ));
    }
}

fn format_persisted_turn_item(item: &crate::execution::PersistedTurnItem) -> String {
    use devo_protocol::native::item::Item;
    use devo_protocol::native::item::UserInput;

    match &item.item {
        Item::UserMessage { content, entry, .. } => {
            let text = content
                .iter()
                .filter_map(|part| match part {
                    UserInput::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            match entry {
                devo_protocol::native::item::UserMessageEntry::Steer => {
                    format!("steer: {text}")
                }
                _ => format!("user: {text}"),
            }
        }
        Item::AssistantMessage { text, .. } => format!("assistant: {text}"),
        Item::Plan { entries, .. } => {
            let text = entries
                .iter()
                .map(|entry| entry.step.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            format!("plan: {text}")
        }
        Item::Reasoning { text, .. } => format!("reasoning: {text}"),
        Item::ToolCall {
            tool_name, input, ..
        } => format!(
            "tool_call {tool_name}: {}",
            input.clone().unwrap_or(serde_json::Value::Null)
        ),
        Item::ToolResult {
            call_id, output, ..
        } => format!("tool_result {call_id}: {output}"),
        Item::CommandExecution { command, .. } => format!("command: {command}"),
        Item::Approval {
            action_summary,
            decision,
            ..
        } => {
            if let Some(decision) = decision {
                format!(
                    "approval_decision: {:?} ({:?})",
                    decision.decision, decision.scope
                )
            } else {
                format!("approval_request: {action_summary}")
            }
        }
        Item::HostedToolCall {
            tool_name, output, ..
        } => {
            let text = output
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("{tool_name}: {text}")
        }
        Item::ContextCompaction { summary, .. } => {
            format!("compaction: {}", summary.as_deref().unwrap_or_default())
        }
        Item::FileChange { call_id, .. } => format!("file_change: {call_id}"),
        Item::UserInputRequest { .. } => "user_input_request".to_string(),
        Item::SubAgent { .. } => "subagent".to_string(),
        Item::BackgroundTask { .. } => "background_task".to_string(),
        Item::GoalProgress { .. } => "goal_progress".to_string(),
        Item::Refinement { .. } => "refinement".to_string(),
        Item::Warning { message, .. } => format!("warning: {message}"),
        Item::BranchSummary { summary, .. } => format!("branch_summary: {summary}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn live_windows_path_prefix_enters_error_arm_before_approval_side_effects() {
        let path = Path::new("approved.txt");
        let result = admit_live_approval_scope(
            true,
            Ok((
                ApprovalDecisionValue::Approve,
                ApprovalScopeValue::PathPrefix,
            )),
            &devo_safety::ResourceKind::FileRead,
            Some(path),
        );
        assert!(result.unwrap_err().contains("unsupported"));
        assert_eq!(
            admit_live_approval_scope(
                true,
                Ok((ApprovalDecisionValue::Approve, ApprovalScopeValue::Once)),
                &devo_safety::ResourceKind::FileRead,
                Some(path),
            )
            .expect("mediated Once"),
            (ApprovalDecisionValue::Approve, ApprovalScopeValue::Once)
        );
    }

    #[test]
    fn approval_policy_strings_map_to_permission_modes() {
        for (policy, expected) in [
            ("on-request", Some(PermissionMode::Interactive)),
            ("never", Some(PermissionMode::Yolo)),
            ("deny", Some(PermissionMode::Deny)),
            ("unknown", None),
        ] {
            assert_eq!(permission_mode_from_approval_policy(policy), expected);
        }
    }

    #[test]
    fn approval_scopes_and_prefix_cache_for_shell_commands() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        cache
            .command_prefixes
            .insert(vec!["git".to_string(), "add".to_string()]);
        let mut request = test_permission_request("shell_command");
        request.command_prefix = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(permission_cache_matches(&cache, &request));
        assert!(
            approval_scopes_for_request(&request)
                .iter()
                .any(|scope| scope == "commandPrefix" || scope == "command_prefix")
        );

        let mut shell_request = test_permission_request("shell_command");
        assert!(
            !approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session")
        );
        shell_request.target = Some("git status".to_string());
        assert!(
            approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session")
        );
        shell_request.command_pattern =
            Some(vec!["git".to_string(), "add".to_string(), "*".to_string()]);
        assert!(
            approval_scopes_for_request(&shell_request)
                .iter()
                .any(|scope| scope == "session")
        );
        assert!(
            !approval_scopes_for_request(&test_permission_request("exec_command"))
                .iter()
                .any(|scope| scope == "session")
        );
        assert!(
            approval_scopes_for_request(&test_permission_request("write"))
                .iter()
                .any(|scope| scope == "session")
        );
    }

    #[test]
    fn command_pattern_cache_allows_matching_argv() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        cache
            .command_patterns
            .insert(vec!["git".to_string(), "add".to_string(), "*".to_string()]);

        let mut request = test_permission_request("shell_command");
        request.command_argv = Some(vec![
            "git".to_string(),
            "add".to_string(),
            "src/main.rs".to_string(),
        ]);
        assert!(permission_cache_matches(&cache, &request));

        request.command_argv = Some(vec!["git".to_string(), "add".to_string()]);
        assert!(
            !permission_cache_matches(&cache, &request),
            "trailing wildcard requires at least one argument"
        );

        request.command_argv = Some(vec![
            "git".to_string(),
            "commit".to_string(),
            "src/main.rs".to_string(),
        ]);
        assert!(!permission_cache_matches(&cache, &request));

        request.command_argv = None;
        assert!(
            !permission_cache_matches(&cache, &request),
            "unsafe commands without screened argv never match patterns"
        );
    }

    #[test]
    fn explicit_escalation_forces_approval() {
        let mut request = test_permission_request("exec_command");
        request.sandbox_permissions = devo_core::tools::SandboxPermissionRequest::FullEscalation;

        assert!(request_forces_approval(&request));
    }

    #[test]
    fn permission_mode_overrides_authorization_policy() {
        assert_eq!(
            permission_mode_authorization(PermissionMode::Yolo),
            Some(AuthorizationDecision::Allow {
                source: devo_protocol::native::item::ApprovalDecisionSource::StaticPolicy,
            })
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Deny),
            Some(AuthorizationDecision::Deny {
                source: devo_protocol::native::item::ApprovalDecisionSource::StaticPolicy,
                reason: "approval policy is deny".to_string(),
            })
        );
        assert_eq!(
            permission_mode_authorization(PermissionMode::Interactive),
            None
        );
    }

    #[test]
    fn yolo_grants_sandbox_bypass_for_escalation() {
        let root = abs_path(&["workspace"]);
        let mut request = test_permission_request("shell_command");
        request.sandbox_permissions = devo_core::tools::SandboxPermissionRequest::FullEscalation;
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });
        request.target = Some("npm install".to_string());
        request.cwd = root.clone();
        let grant = PermissionGrant {
            bypass_sandbox: true,
            already_approved: false,
            sandbox_permission_overlay: None,
        };

        assert_eq!(
            permission_mode_authorization(PermissionMode::Yolo),
            Some(AuthorizationDecision::Allow {
                source: devo_protocol::native::item::ApprovalDecisionSource::StaticPolicy,
            })
        );
        assert_eq!(escalation_permission_grant(&request), grant);

        let mut profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root,
        );
        profile.yolo = true;
        assert!(matches!(
            test_policy_decision(&profile, &request),
            AuthorizationDecision::Allow { .. }
        ));
    }

    #[test]
    fn blocking_append_allow_prefix_rule_writes_rule_line() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let policy_path = tmp.path().join("rules").join("default.rules");
        let prefix = vec!["git".to_string(), "pull".to_string()];

        devo_execpolicy::blocking_append_allow_prefix_rule(&policy_path, &prefix)
            .expect("append rule");

        let contents = std::fs::read_to_string(&policy_path).expect("read rules");
        assert_eq!(
            contents,
            r#"prefix_rule(pattern=["git", "pull"], decision="allow")
"#
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
    fn file_tool_cache_scope_matches_expected_paths() {
        let root = abs_path(&["workspace", "src"]);
        let granted = root.join("main.rs");
        let sibling = root.join("helper.rs");
        let outside = abs_path(&["workspace", "other.rs"]);

        let mut exact_cache = crate::execution::ApprovalGrantCache::default();
        exact_cache
            .write_exact_paths
            .insert(normalize_permission_path(&granted));
        let mut exact_allowed = test_permission_request("write");
        exact_allowed.resource = devo_safety::ResourceKind::FileWrite;
        exact_allowed.path = Some(granted.clone());
        let mut exact_sibling = exact_allowed.clone();
        exact_sibling.path = Some(sibling.clone());
        let mut exact_read = test_permission_request("read");
        exact_read.resource = devo_safety::ResourceKind::FileRead;
        exact_read.path = Some(granted.clone());
        assert!(permission_cache_matches(&exact_cache, &exact_allowed));
        assert!(!permission_cache_matches(&exact_cache, &exact_sibling));
        assert!(!permission_cache_matches(&exact_cache, &exact_read));

        let mut prefix_cache = crate::execution::ApprovalGrantCache::default();
        prefix_cache
            .write_path_prefixes
            .insert(path_prefix_grant_root(&granted));
        let mut prefix_sibling = test_permission_request("edit");
        prefix_sibling.resource = devo_safety::ResourceKind::FileWrite;
        prefix_sibling.path = Some(sibling);
        let mut prefix_outside = test_permission_request("write");
        prefix_outside.resource = devo_safety::ResourceKind::FileWrite;
        prefix_outside.path = Some(outside);
        assert!(permission_cache_matches(&prefix_cache, &exact_allowed));
        assert!(permission_cache_matches(&prefix_cache, &prefix_sibling));
        assert!(!permission_cache_matches(&prefix_cache, &prefix_outside));
    }

    #[test]
    fn approval_decision_from_acp_maps_file_tool_scopes() {
        for (option_id, expected) in [
            (
                "allow_once",
                (ApprovalDecisionValue::Approve, ApprovalScopeValue::Once),
            ),
            (
                "allow_session",
                (ApprovalDecisionValue::Approve, ApprovalScopeValue::Session),
            ),
            (
                "allow_path_prefix",
                (
                    ApprovalDecisionValue::Approve,
                    ApprovalScopeValue::PathPrefix,
                ),
            ),
            (
                "reject_once",
                (ApprovalDecisionValue::Deny, ApprovalScopeValue::Once),
            ),
        ] {
            assert_eq!(
                approval_decision_from_acp_outcome(devo_protocol::AcpPermissionOutcome::Selected {
                    option_id: option_id.to_string(),
                }),
                Ok(expected)
            );
        }
    }

    #[test]
    fn approval_path_cache_does_not_allow_parent_escape() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let root = abs_path(&["workspace", "generated"]);
        cache.write_path_prefixes.insert(root.clone());

        let mut escaped = test_permission_request("write");
        escaped.resource = devo_safety::ResourceKind::FileWrite;
        escaped.path = Some(root.join("..").join("outside.txt"));

        let mut allowed = test_permission_request("write");
        allowed.resource = devo_safety::ResourceKind::FileWrite;
        allowed.path = Some(root.join("..").join("generated").join("file.txt"));

        assert!(!permission_cache_matches(&cache, &escaped));
        assert!(permission_cache_matches(&cache, &allowed));

        let mut read_only = test_permission_request("read");
        read_only.resource = devo_safety::ResourceKind::FileRead;
        read_only.path = Some(root.join("file.txt"));
        assert!(
            !permission_cache_matches(&cache, &read_only),
            "write path grants must not auto-allow reads via write cache"
        );
    }

    #[test]
    fn policy_decisions_for_file_and_shell_requests() {
        let (root, profile) = workspace_profile();
        let mut inside_read = test_permission_request("read");
        inside_read.resource = devo_safety::ResourceKind::FileRead;
        inside_read.path = Some(root.join("Cargo.toml"));
        assert!(matches!(
            test_policy_decision(&profile, &inside_read),
            AuthorizationDecision::Allow { .. }
        ));

        let mut outside_read = test_permission_request("read");
        outside_read.resource = devo_safety::ResourceKind::FileRead;
        outside_read.path = Some(abs_path(&["outside", "secret.txt"]));
        assert!(matches!(
            test_policy_decision(&profile, &outside_read),
            AuthorizationDecision::Ask { .. }
        ));

        let mut outside_redirect = test_permission_request("shell_command");
        outside_redirect.target = Some(format!(
            "cat > {}/outside.txt",
            abs_path(&["etc"]).display()
        ));
        outside_redirect.input = serde_json::json!({ "command": outside_redirect.target });
        assert!(matches!(
            test_policy_decision(&profile, &outside_redirect),
            AuthorizationDecision::Ask { .. }
        ));

        let file_path = root
            .join("file.txt")
            .display()
            .to_string()
            .replace('\\', "/");
        let mut inside_redirect = test_permission_request("shell_command");
        inside_redirect.target = Some(format!("cat > '{file_path}'"));
        inside_redirect.input = serde_json::json!({ "command": inside_redirect.target });
        inside_redirect.cwd = root.clone();
        assert!(matches!(
            test_policy_decision(&profile, &inside_redirect),
            AuthorizationDecision::Allow { .. }
        ));

        for (command, expect_allow) in [
            ("git status", true),
            ("rm -f important.txt", false),
            ("sleep 1 & touch evil", false),
            ("sleep 1& rm x", false),
            (r#"echo "http://x/?a=1&b=2""#, true),
        ] {
            let mut request = test_permission_request("shell_command");
            request.target = Some(command.to_string());
            request.input = serde_json::json!({ "command": command });
            request.cwd = root.clone();
            let decision = test_policy_decision(&profile, &request);
            if expect_allow {
                assert!(
                    matches!(decision, AuthorizationDecision::Allow { .. }),
                    "expected Allow for {command}"
                );
            } else {
                assert!(
                    matches!(decision, AuthorizationDecision::Ask { .. }),
                    "expected Ask for {command}"
                );
            }
        }
    }

    #[test]
    fn sandbox_bypass_cache_matches_exact_escalation_command() {
        let mut cache = crate::execution::ApprovalGrantCache::default();
        let mut request = test_permission_request("shell_command");
        request.input = serde_json::json!({
            "command": "npm install",
            "sandbox_permissions": "require_escalated"
        });
        request.target = Some("npm install".to_string());
        request.sandbox_permissions = devo_core::tools::SandboxPermissionRequest::FullEscalation;
        let key = sandbox_bypass_key_from_request(&request).expect("bypass key");
        cache.sandbox_bypass_commands.insert(key);

        assert_eq!(
            cache_grant(&cache, &request),
            Some(PermissionGrant {
                bypass_sandbox: true,
                already_approved: true,
                sandbox_permission_overlay: None,
            })
        );
        request.target = Some("npm ci".to_string());
        assert_eq!(cache_grant(&cache, &request), None);
    }

    fn test_policy_decision(
        profile: &devo_safety::RuntimePermissionProfile,
        request: &ToolPermissionRequest,
    ) -> AuthorizationDecision {
        policy_decision(profile, request, /*exec_policy*/ None)
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
            command_argv: None,
            command_pattern: None,
            sandbox_permissions: devo_core::tools::SandboxPermissionRequest::Default,
        }
    }
    fn workspace_profile() -> (PathBuf, devo_safety::RuntimePermissionProfile) {
        let root = abs_path(&["workspace"]);
        let profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            root.clone(),
        );
        (root, profile)
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

    #[test]
    fn p3_rule_lines_are_stable_and_idempotent_shaped() {
        // The durable rule text is the wire contract between sessions —
        // lock its shape (goal rule 7).
        let write_line = super::ServerRuntime::path_prefix_rule_line(
            std::path::Path::new(r"C:\Users\me\proj"),
            true,
        );
        assert!(
            write_line.starts_with("path_prefix allow write "),
            "{write_line}"
        );
        let read_line = super::ServerRuntime::path_prefix_rule_line(
            std::path::Path::new("/home/me/proj"),
            false,
        );
        assert!(
            read_line.starts_with("path_prefix allow read /"),
            "{read_line}"
        );
        let host_line = super::ServerRuntime::host_rule_line("api.example.com");
        assert_eq!(host_line, "network_rule allow api.example.com\n");
    }
}
