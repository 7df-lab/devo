use super::super::*;

use std::collections::BTreeSet;

use chrono::DateTime;

use crate::AcpErrorCode;
use crate::AcpSetConfigOptionParams;
use crate::execution::RuntimeSession;
use devo_protocol::AcpSessionConfigOption;
use devo_protocol::AcpSessionConfigOptionCategory;
use devo_protocol::AcpSessionConfigOptionCategoryKnown;
use devo_protocol::AcpSessionConfigSelectOption;
use devo_protocol::AcpSessionConfigSelectOptions;
use devo_protocol::Model;
use devo_protocol::PermissionPreset;
use devo_protocol::SessionId;

const ACP_MODE_CONFIG_ID: &str = "mode";
const ACP_MODEL_CONFIG_ID: &str = "model";
const ACP_REASONING_EFFORT_CONFIG_ID: &str = "thought_level";

impl ServerRuntime {
    pub(super) async fn acp_session_config_options(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<AcpSessionConfigOption>, String> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        let session = session_arc.lock().await;
        Ok(acp_config_options_for_session(&session))
    }

    pub(super) async fn set_acp_session_config_option(
        &self,
        params: AcpSetConfigOptionParams,
    ) -> Result<Vec<AcpSessionConfigOption>, (AcpErrorCode, String)> {
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return Err((
                AcpErrorCode::ServerError,
                "session does not exist".to_string(),
            ));
        };

        let (updated_session, config_options) = {
            let mut session = session_arc.lock().await;
            match params.config_id.as_str() {
                ACP_MODEL_CONFIG_ID => {
                    let model_option = acp_model_config_option_for_session(&session);
                    let value_is_allowed = match &model_option {
                        AcpSessionConfigOption::Select { options, .. } => {
                            select_options_contain_value(options, &params.value)
                        }
                    };
                    if !value_is_allowed {
                        return Err((
                            AcpErrorCode::InvalidParams,
                            format!(
                                "invalid value '{}' for session config option '{}'",
                                params.value, params.config_id
                            ),
                        ));
                    }

                    let mut turn_config = session.runtime_context.resolve_turn_config(
                        Some(params.value.as_str()),
                        session.summary.reasoning_effort_selection.clone(),
                    );
                    turn_config.reasoning_effort_selection = current_reasoning_effort_value(
                        &turn_config.model,
                        session.summary.reasoning_effort_selection.as_deref(),
                    );
                    apply_turn_config_to_session_summary(&mut session.summary, &turn_config);
                    let updated_at = Utc::now();
                    session.summary.updated_at = updated_at;
                    if let Err(error) = persist_session_config_summary(
                        &self.rollout_store,
                        &mut session,
                        updated_at,
                    ) {
                        return Err((AcpErrorCode::ServerError, error));
                    }
                    (
                        Some(session.summary.clone()),
                        acp_config_options_for_session(&session),
                    )
                }
                ACP_REASONING_EFFORT_CONFIG_ID => {
                    let Some(reasoning_effort_option) =
                        acp_reasoning_effort_config_option_for_session(&session)
                    else {
                        return Err((
                            AcpErrorCode::InvalidParams,
                            format!("unknown session config option '{}'", params.config_id),
                        ));
                    };
                    let value_is_allowed = match &reasoning_effort_option {
                        AcpSessionConfigOption::Select { options, .. } => {
                            select_options_contain_value(options, &params.value)
                        }
                    };
                    if !value_is_allowed {
                        return Err((
                            AcpErrorCode::InvalidParams,
                            format!(
                                "invalid value '{}' for session config option '{}'",
                                params.value, params.config_id
                            ),
                        ));
                    }

                    let mut turn_config = session.runtime_context.resolve_turn_config(
                        session_model_selection(&session.summary),
                        Some(params.value.clone()),
                    );
                    turn_config.reasoning_effort_selection = Some(params.value.clone());
                    apply_turn_config_to_session_summary(&mut session.summary, &turn_config);
                    let updated_at = Utc::now();
                    session.summary.updated_at = updated_at;
                    if let Err(error) = persist_session_config_summary(
                        &self.rollout_store,
                        &mut session,
                        updated_at,
                    ) {
                        return Err((AcpErrorCode::ServerError, error));
                    }
                    (
                        Some(session.summary.clone()),
                        acp_config_options_for_session(&session),
                    )
                }
                ACP_MODE_CONFIG_ID => {
                    let Some(preset) = permission_preset_from_value(&params.value) else {
                        return Err((
                            AcpErrorCode::InvalidParams,
                            format!(
                                "invalid value '{}' for session config option '{}'",
                                params.value, params.config_id
                            ),
                        ));
                    };
                    let profile = safety_profile_from_protocol(
                        preset,
                        session.summary.cwd.clone(),
                        session.summary.additional_directories.clone(),
                    );
                    {
                        let mut core_session = session.core_session.lock().await;
                        core_session.config.permission_mode = profile.permission_mode();
                        core_session.config.permission_profile = profile.clone();
                    }
                    session.config.permission_mode = profile.permission_mode();
                    session.config.permission_profile = profile;
                    session.session_approval_cache =
                        crate::execution::ApprovalGrantCache::default();
                    session.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
                    (None, acp_config_options_for_session(&session))
                }
                _ => {
                    return Err((
                        AcpErrorCode::InvalidParams,
                        format!("unknown session config option '{}'", params.config_id),
                    ));
                }
            }
        };

        if let Some(updated_session) = updated_session
            && !updated_session.ephemeral
            && let Err(error) = self.deps.db.upsert_session(&updated_session)
        {
            tracing::warn!(
                session_id = %params.session_id,
                %error,
                "failed to persist ACP session config option"
            );
        }

        Ok(config_options)
    }
}

fn acp_config_options_for_session(session: &RuntimeSession) -> Vec<AcpSessionConfigOption> {
    let mut options = vec![acp_model_config_option_for_session(session)];
    if let Some(reasoning_effort_option) = acp_reasoning_effort_config_option_for_session(session) {
        options.push(reasoning_effort_option);
    }
    options.push(acp_mode_config_option_for_session(session));
    options
}

fn acp_mode_config_option_for_session(session: &RuntimeSession) -> AcpSessionConfigOption {
    let current_value = permission_preset_value(permission_preset_from_safety(
        session.config.permission_profile.preset,
    ))
    .to_string();
    AcpSessionConfigOption::Select {
        id: ACP_MODE_CONFIG_ID.to_string(),
        name: "Session Mode".to_string(),
        description: Some("Controls how Devo requests permission".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::Mode,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(
            [
                (
                    PermissionPreset::ReadOnly,
                    "Read Only",
                    "Read workspace files without approval. Ask before edits, commands, or network access.",
                ),
                (
                    PermissionPreset::Default,
                    "Default",
                    "Read, edit workspace files, and run commands. Ask before network access or outside-workspace edits.",
                ),
                (
                    PermissionPreset::AutoReview,
                    "Auto-review",
                    "Use default workspace permissions and route eligible approvals through auto-review.",
                ),
                (
                    PermissionPreset::FullAccess,
                    "Full Access",
                    "Allow all tool requests without approval.",
                ),
            ]
            .into_iter()
            .map(|(preset, name, description)| AcpSessionConfigSelectOption {
                value: permission_preset_value(preset).to_string(),
                name: name.to_string(),
                description: Some(description.to_string()),
                meta: None,
            })
            .collect(),
        ),
    }
}

fn acp_model_config_option_for_session(session: &RuntimeSession) -> AcpSessionConfigOption {
    let turn_config = session.runtime_context.resolve_turn_config(
        session_model_selection(&session.summary),
        session.summary.reasoning_effort_selection.clone(),
    );
    let current_value = turn_config
        .model_binding_id
        .clone()
        .unwrap_or_else(|| turn_config.model.slug.clone());
    let config = session
        .runtime_context
        .config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .clone();

    let mut options = Vec::new();
    let mut seen_values = BTreeSet::new();
    for (binding_id, binding) in &config.provider.model_bindings {
        if !binding.enabled || !seen_values.insert(binding_id.clone()) {
            continue;
        }
        let model_display_name = session
            .runtime_context
            .model_catalog
            .get(&binding.model_slug)
            .map(|model| model.display_name.as_str())
            .and_then(non_empty_str);
        let name = binding
            .display_name
            .as_deref()
            .and_then(non_empty_str)
            .or(model_display_name)
            .unwrap_or(binding.model_slug.as_str())
            .to_string();
        let provider_name = config
            .provider
            .providers
            .get(&binding.provider)
            .map(|provider| provider.name.as_str())
            .and_then(non_empty_str)
            .unwrap_or(binding.provider.as_str());
        options.push(AcpSessionConfigSelectOption {
            value: binding_id.clone(),
            name,
            description: Some(format!(
                "{provider_name}: {} via {}",
                binding.model_name, binding.invocation_method
            )),
            meta: None,
        });
    }

    if !seen_values.contains(&current_value) {
        options.insert(
            0,
            AcpSessionConfigSelectOption {
                value: current_value.clone(),
                name: current_value.clone(),
                description: None,
                meta: None,
            },
        );
    }

    AcpSessionConfigOption::Select {
        id: ACP_MODEL_CONFIG_ID.to_string(),
        name: "Model".to_string(),
        description: Some("Controls the model used for this session".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::Model,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(options),
    }
}

fn acp_reasoning_effort_config_option_for_session(
    session: &RuntimeSession,
) -> Option<AcpSessionConfigOption> {
    let turn_config = session.runtime_context.resolve_turn_config(
        session_model_selection(&session.summary),
        session.summary.reasoning_effort_selection.clone(),
    );
    let current_value = current_reasoning_effort_value(
        &turn_config.model,
        session.summary.reasoning_effort_selection.as_deref(),
    )?;
    let mut seen_values = BTreeSet::new();
    let mut options = Vec::new();
    for preset in turn_config.model.effective_reasoning_capability().options() {
        if !seen_values.insert(preset.value.clone()) {
            continue;
        }
        options.push(AcpSessionConfigSelectOption {
            value: preset.value,
            name: preset.label,
            description: Some(preset.description),
            meta: None,
        });
    }

    if options.is_empty() {
        return None;
    }

    if !seen_values.contains(&current_value) {
        options.insert(
            0,
            AcpSessionConfigSelectOption {
                value: current_value.clone(),
                name: current_value.clone(),
                description: None,
                meta: None,
            },
        );
    }

    Some(AcpSessionConfigOption::Select {
        id: ACP_REASONING_EFFORT_CONFIG_ID.to_string(),
        name: "Reasoning Effort".to_string(),
        description: Some("Controls the model reasoning effort used for this session".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::ThoughtLevel,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(options),
    })
}

fn current_reasoning_effort_value(model: &Model, selection: Option<&str>) -> Option<String> {
    let option_values = model
        .effective_reasoning_capability()
        .options()
        .into_iter()
        .map(|option| option.value)
        .collect::<Vec<_>>();
    if option_values.is_empty() {
        return None;
    }
    model
        .normalize_reasoning_effort_selection(selection)
        .filter(|value| option_values.contains(value))
        .or_else(|| {
            model
                .default_reasoning_effort_selection()
                .filter(|value| option_values.contains(value))
        })
        .or_else(|| option_values.first().cloned())
}

fn persist_session_config_summary(
    rollout_store: &crate::persistence::RolloutStore,
    session: &mut RuntimeSession,
    updated_at: DateTime<Utc>,
) -> Result<(), String> {
    let model = session.summary.model.clone();
    let model_binding_id = session.summary.model_binding_id.clone();
    let reasoning_effort_selection = session.summary.reasoning_effort_selection.clone();
    if let Some(record) = session.record.as_mut() {
        record.model = model;
        record.model_binding_id = model_binding_id;
        record.reasoning_effort_selection = reasoning_effort_selection;
        record.updated_at = updated_at;
        if let Err(error) = rollout_store.append_session_meta(record) {
            return Err(format!(
                "failed to persist ACP session config option: {error}"
            ));
        }
    }
    Ok(())
}

fn select_options_contain_value(options: &AcpSessionConfigSelectOptions, value: &str) -> bool {
    match options {
        AcpSessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().any(|option| option.value == value)
        }
        AcpSessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .any(|option| option.value == value),
    }
}

fn non_empty_str(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn permission_preset_value(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::ReadOnly => "read-only",
        PermissionPreset::Default => "default",
        PermissionPreset::AutoReview => "auto-review",
        PermissionPreset::FullAccess => "full-access",
    }
}

fn permission_preset_from_value(value: &str) -> Option<PermissionPreset> {
    match value {
        "read-only" => Some(PermissionPreset::ReadOnly),
        "default" => Some(PermissionPreset::Default),
        "auto-review" => Some(PermissionPreset::AutoReview),
        "full-access" => Some(PermissionPreset::FullAccess),
        _ => None,
    }
}

fn permission_preset_from_safety(preset: devo_safety::PermissionPreset) -> PermissionPreset {
    match preset {
        devo_safety::PermissionPreset::ReadOnly => PermissionPreset::ReadOnly,
        devo_safety::PermissionPreset::Default => PermissionPreset::Default,
        devo_safety::PermissionPreset::AutoReview => PermissionPreset::AutoReview,
        devo_safety::PermissionPreset::FullAccess => PermissionPreset::FullAccess,
    }
}
