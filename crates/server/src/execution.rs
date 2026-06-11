use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::AppConfigStore;
use devo_core::ProviderRequestModelMap;
use devo_core::ProviderVendorCatalog;
use futures::Stream;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::AgentsMdConfig;
use devo_core::AppConfig;
use devo_core::Model;
use devo_core::ModelCatalog;
use devo_core::ResolvedSkill;
use devo_core::ResolvedWebFetchConfig;
use devo_core::ResolvedWebSearchConfig;
use devo_core::SessionConfig;
use devo_core::SessionId;
use devo_core::SessionRecord;
use devo_core::SessionState;
use devo_core::SkillCatalog;
use devo_core::SkillError;
use devo_core::SkillSelector;
use devo_core::TurnConfig;
use devo_core::TurnId;
use devo_core::WebFetchConfig;
use devo_core::WebSearchConfig;
use devo_core::default_base_instructions;
use devo_core::normalize_canonical_path;
use devo_core::provider_request_model_map_for_binding;
use devo_core::read_user_auth_config;
use devo_core::resolve_enabled_model_binding;
use devo_core::resolve_web_fetch_config;
use devo_core::resolve_web_search_config;
use devo_core::tools::ToolRegistry;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::PendingInputItem;
use devo_protocol::RequestUserInputResponse;
use devo_protocol::SkillDependencies as ProtocolSkillDependencies;
use devo_protocol::SkillInterface as ProtocolSkillInterface;
use devo_protocol::SkillScope as ProtocolSkillScope;
use devo_protocol::SkillToolDependency as ProtocolSkillToolDependency;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;

use crate::InputItem;
use crate::SkillRecord;
use crate::db::Database;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
use crate::turn::TurnMetadata;

#[derive(Debug, Clone)]
pub(crate) struct PersistedTurnItem {
    pub(crate) turn_id: TurnId,
    pub(crate) item_id: devo_core::ItemId,
    pub(crate) turn_item: devo_core::TurnItem,
}

pub(crate) struct PendingApproval {
    pub(crate) turn_id: TurnId,
    pub(crate) tool_name: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) command_prefix: Option<Vec<String>>,
    pub(crate) tx: oneshot::Sender<ApprovalDecisionValue>,
}

pub(crate) struct PendingUserInput {
    pub(crate) turn_id: TurnId,
    pub(crate) tx: oneshot::Sender<RequestUserInputResponse>,
}

#[derive(Default)]
pub(crate) struct ApprovalGrantCache {
    pub(crate) tools: HashSet<String>,
    pub(crate) hosts: HashSet<String>,
    pub(crate) path_prefixes: HashSet<PathBuf>,
    pub(crate) command_prefixes: HashSet<Vec<String>>,
}

struct RoutedModelProvider {
    router: Arc<dyn ProviderRouter>,
    route: ProviderRoute,
}

impl RoutedModelProvider {
    fn new(router: Arc<dyn ProviderRouter>, route: ProviderRoute) -> Self {
        Self { router, route }
    }
}

#[async_trait::async_trait]
impl ModelProviderSDK for RoutedModelProvider {
    async fn completion(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        self.router
            .complete(self.route.clone(), request)
            .await
            .map_err(anyhow::Error::new)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        self.router
            .stream(self.route.clone(), request)
            .await
            .map_err(anyhow::Error::new)
    }

    fn name(&self) -> &str {
        self.router.name()
    }
}

/// Shared server-owned runtime dependencies used by live turn execution.
pub struct ServerRuntimeDependencies {
    /// Provider used for all model requests.
    /// TODO: the `Arc<dyn ModelProviderSDK>` is one of {OpenAI Chat Completion, OpenAI Responses, Anthropic Messages}, should change it to hash map, as ModelProviderSdkRegistry
    pub(crate) provider: Arc<dyn ModelProviderSDK>,
    /// TODO: the router method is, take the binding of model and provider, then decide which ModelProviderSdk to call. so, let's move this functionality to ModelProviderSdkRegistry, as a method.
    /// Provider router facade for model invocation dispatch.
    #[allow(dead_code)]
    pub(crate) provider_router: Arc<dyn ProviderRouter>,
    /// Shared built-in tool registry used by turn execution.
    pub(crate) registry: Arc<ToolRegistry>,
    /// TODO: Should we have this? If there is no valid model as configuration file, then throw error,
    /// exit the program, hint user to add a valid model at configuration file, or onboard.
    /// Default model applied when no model override is present.
    ///
    /// This is guaranteed by server bootstrap and used as the fallback model
    /// when session or turn metadata does not specify one.
    pub(crate) default_model: String,
    /// ProviderVendor catalog used to resolve current provider.
    #[allow(dead_code)]
    pub(crate) provider_vendor_catalog: Arc<ProviderVendorCatalog>,
    /// Model catalog used to resolve builtin prompt metadata.
    pub(crate) model_catalog: Arc<dyn ModelCatalog>,
    /// Default workspace root used for workspace-scoped skill discovery.
    pub(crate) skill_workspace_root: Option<PathBuf>,
    /// Skill catalog for discovering and loading skills.
    pub(crate) skill_catalog: StdMutex<Box<dyn SkillCatalog + Send>>,
    /// AGENTS.md/PROMPT.md/CLAUDE.md discovery configuration applied to new sessions.
    pub(crate) agents_md: AgentsMdConfig,
    /// SQLite database for session metadata, token stats, and pending queues.
    pub(crate) db: Arc<Database>,
    /// Shared app config loaded from user and optional workspace config files.
    pub(crate) config_store: Arc<std::sync::Mutex<AppConfigStore>>,
}

impl ServerRuntimeDependencies {
    /// Creates a new bundle of runtime dependencies for the transport server.
    /// TODO: Should fix the clippy::too_many_arguments, decrease the arguments count.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn ModelProviderSDK>,
        provider_router: Arc<dyn ProviderRouter>,
        registry: Arc<ToolRegistry>,
        default_model: String,
        model_catalog: Arc<dyn ModelCatalog>,
        provider_vendor_catalog: Arc<ProviderVendorCatalog>,
        skill_workspace_root: Option<PathBuf>,
        skill_catalog: Box<dyn SkillCatalog + Send>,
        agents_md: AgentsMdConfig,
        db: Arc<Database>,
        config_store: Arc<std::sync::Mutex<AppConfigStore>>,
    ) -> Self {
        Self {
            provider,
            provider_router,
            registry,
            default_model,
            model_catalog,
            provider_vendor_catalog,
            skill_workspace_root,
            skill_catalog: StdMutex::new(skill_catalog),
            agents_md,
            db,
            config_store,
        }
    }

    pub(crate) fn provider_for_route(&self, route: ProviderRoute) -> Arc<dyn ModelProviderSDK> {
        Arc::new(RoutedModelProvider::new(
            Arc::clone(&self.provider_router),
            route,
        ))
    }

    /// Creates an initial core session state for a newly created server session.
    pub(crate) fn new_session_state(&self, session_id: SessionId, cwd: PathBuf) -> SessionState {
        // TODO: Session config already has workspace cwd, I think the cwd at permission_profile preset is duplicated.
        let permission_profile = devo_safety::RuntimePermissionProfile::from_preset(
            devo_safety::PermissionPreset::Default,
            cwd.clone(),
        );
        let mut state = SessionState::new(
            SessionConfig {
                permission_mode: permission_profile.permission_mode(),
                permission_profile,
                agents_md: self.agents_md.clone(),
                ..SessionConfig::default()
            },
            cwd,
        );
        state.id = session_id.to_string();
        state
    }

    fn catalog_model_or_fallback(&self, model_slug: &str) -> Model {
        self.model_catalog
            .get(model_slug)
            .cloned()
            .unwrap_or_else(|| Model {
                slug: model_slug.to_string(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            })
    }

    /// Resolves one runtime model for a turn, applying the server default when needed.
    pub(crate) fn resolve_turn_model(&self, requested_model: Option<&str>) -> Model {
        if let Some(model) = requested_model.and_then(|requested| self.model_catalog.get(requested))
        {
            return model.clone();
        }

        self.model_catalog
            .resolve_for_turn(Some(&self.default_model))
            .or_else(|_| self.model_catalog.resolve_for_turn(None))
            .cloned()
            .unwrap_or_else(|_| Model {
                slug: self.default_model.clone(),
                base_instructions: default_base_instructions().to_string(),
                ..Model::default()
            })
    }

    /// Resolves the full turn configuration used by the core query loop.
    pub(crate) fn resolve_turn_config(
        &self,
        requested_model: Option<&str>,
        thinking_selection: Option<String>,
    ) -> TurnConfig {
        let (config, user_config_dir) = {
            let config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            (
                config_store.effective_config().clone(),
                config_store.user_config_dir().to_path_buf(),
            )
        };
        let provider_config = config.provider.clone();

        if let Some(binding) = resolve_enabled_model_binding(&provider_config, requested_model) {
            let provider = provider_config.providers.get(&binding.provider_id);
            let binding_config = provider_config.model_bindings.get(&binding.binding_id);
            let web_search = self.resolve_turn_web_search(
                &config,
                &user_config_dir,
                provider.and_then(|provider| provider.web_search.as_ref()),
                binding_config.and_then(|binding| binding.web_search.as_ref()),
            );
            let web_fetch = self.resolve_turn_web_fetch(
                &config,
                provider.and_then(|provider| provider.web_fetch.as_ref()),
                binding_config.and_then(|binding| binding.web_fetch.as_ref()),
            );
            // Variant request models are scoped to the selected provider. If
            // both OpenRouter and a custom provider configure
            // `model_slug = "kimi-k2.5-thinking"`, a turn selected through
            // OpenRouter must use OpenRouter's `model_name`.
            let provider_request_models = ProviderRequestModelMap::new(
                provider_request_model_map_for_binding(&provider_config, &binding),
            );
            let binding_id = binding.binding_id.clone();
            let mut turn_config = TurnConfig::with_provider_route_and_web_tools(
                self.catalog_model_or_fallback(&binding.model_slug),
                binding.model_name,
                provider_request_models,
                ProviderRoute::binding(binding.provider_id, binding.invocation_method),
                web_search,
                web_fetch,
                thinking_selection,
            );
            turn_config.model_binding_id = Some(binding_id);
            return turn_config;
        }

        let model = self.resolve_turn_model(requested_model);
        let web_search = self.resolve_turn_web_search(&config, &user_config_dir, None, None);
        let web_fetch = self.resolve_turn_web_fetch(&config, None, None);
        let mut turn_config = TurnConfig::new(model, thinking_selection);
        turn_config.web_search = web_search;
        turn_config.web_fetch = web_fetch;
        turn_config
    }

    fn resolve_turn_web_search(
        &self,
        config: &AppConfig,
        user_config_dir: &Path,
        provider_override: Option<&WebSearchConfig>,
        binding_override: Option<&WebSearchConfig>,
    ) -> ResolvedWebSearchConfig {
        let auth = match read_user_auth_config(&user_config_dir.join(AUTH_CONFIG_FILE_NAME)) {
            Ok(auth) => auth,
            Err(error) => {
                tracing::warn!(%error, "failed to load web_search credentials");
                return ResolvedWebSearchConfig::Disabled;
            }
        };
        match resolve_web_search_config(
            &config.tools.web_search,
            provider_override,
            binding_override,
            &auth,
        ) {
            Ok(web_search) => web_search,
            Err(error) => {
                tracing::warn!(%error, "failed to resolve web_search config");
                ResolvedWebSearchConfig::Disabled
            }
        }
    }

    fn resolve_turn_web_fetch(
        &self,
        config: &AppConfig,
        provider_override: Option<&WebFetchConfig>,
        binding_override: Option<&WebFetchConfig>,
    ) -> ResolvedWebFetchConfig {
        resolve_web_fetch_config(&config.tools.web_fetch, provider_override, binding_override)
    }

    /// Should move the discover skill main logic to skills crate, and server just keep a simple wrapper.
    /// Returns the current skill catalog snapshot for one optional workspace root.
    pub(crate) fn discover_skills(
        &self,
        workspace_root: Option<&Path>,
        force_reload: bool,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        let workspace_root = workspace_root.or(self.skill_workspace_root.as_deref());
        let mut skill_catalog = self
            .skill_catalog
            .lock()
            .expect("skill catalog mutex should not be poisoned");
        skill_catalog
            .discover(workspace_root, force_reload)
            .map(|skills| {
                skills
                    .into_iter()
                    .map(core_skill_record_to_protocol)
                    .collect()
            })
    }

    pub(crate) fn set_skill_enabled(
        &self,
        path: PathBuf,
        enabled: bool,
        workspace_root: Option<&Path>,
    ) -> anyhow::Result<Vec<SkillRecord>> {
        let (skills_config, project_root_markers) = {
            let mut config_store = self
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            config_store.set_skill_enabled(path, enabled)?;
            let effective = config_store.effective_config();
            (
                effective.skills.clone(),
                effective.project_root_markers.clone(),
            )
        };

        {
            let mut skill_catalog = self
                .skill_catalog
                .lock()
                .expect("skill catalog mutex should not be poisoned");
            skill_catalog.set_config(skills_config, project_root_markers);
        }

        self.discover_skills(workspace_root, true)
            .map_err(|error| anyhow::anyhow!(error))
    }

    /// Renders turn input items and resolves any referenced skills into prompt-visible text.
    pub(crate) fn resolve_input_items(
        &self,
        input: &[InputItem],
        workspace_root: Option<&Path>,
    ) -> Result<Option<String>, SkillError> {
        let workspace_root = workspace_root.or(self.skill_workspace_root.as_deref());
        let mut skill_catalog = self
            .skill_catalog
            .lock()
            .expect("skill catalog mutex should not be poisoned");
        let discovered_skills = skill_catalog.discover(workspace_root, false)?;

        let mut parts = Vec::new();
        if let Some(instructions) =
            skill_catalog.available_skills_instructions(workspace_root, None)?
        {
            parts.push(instructions.trim().to_string());
        }

        let structured_skill_names = input
            .iter()
            .filter_map(|item| match item {
                InputItem::Skill { name, .. } => Some(name.clone()),
                InputItem::Text { .. }
                | InputItem::LocalImage { .. }
                | InputItem::Mention { .. } => None,
            })
            .collect::<HashSet<_>>();
        let mut injected_plain_skill_paths = HashSet::new();
        for text in input.iter().filter_map(|item| match item {
            InputItem::Text { text } => Some(text),
            InputItem::Skill { .. } | InputItem::LocalImage { .. } | InputItem::Mention { .. } => {
                None
            }
        }) {
            for name in plain_skill_mentions(text) {
                if structured_skill_names.contains(&name) {
                    continue;
                }
                let matches = discovered_skills
                    .iter()
                    .filter(|skill| skill.name == name)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [skill] if skill.enabled => {
                        if injected_plain_skill_paths.insert(skill.path.clone()) {
                            let rendered = skill_catalog
                                .load(
                                    &SkillSelector {
                                        name: skill.name.clone(),
                                        path: Some(skill.path.clone()),
                                    },
                                    workspace_root,
                                )
                                .map(|skill| render_resolved_skill(&skill))?;
                            parts.push(rendered);
                        }
                    }
                    [skill] => {
                        return Err(SkillError::SkillDisabled {
                            name: skill.name.clone(),
                            path: skill.path.clone(),
                        });
                    }
                    [] => {}
                    _ => {
                        return Err(SkillError::AmbiguousSkillName {
                            name,
                            paths: matches.iter().map(|skill| skill.path.clone()).collect(),
                        });
                    }
                }
            }
        }

        let item_parts = input
            .iter()
            .map(|item| match item {
                InputItem::Text { text } => Ok(text.trim().to_string()),
                InputItem::Skill { name, path } => skill_catalog
                    .load(
                        &SkillSelector {
                            name: name.clone(),
                            path: (!path.as_os_str().is_empty()).then(|| path.clone()),
                        },
                        workspace_root,
                    )
                    .map(|skill| render_resolved_skill(&skill)),
                InputItem::LocalImage { path } => Ok(format!("[image:{}]", path.display())),
                InputItem::Mention { path, name } => Ok(format!(
                    "[mention:{}]",
                    name.as_deref().unwrap_or(path.as_str())
                )),
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        parts.extend(item_parts);
        Ok((!parts.is_empty()).then(|| parts.join("\n")))
    }
}

fn core_skill_record_to_protocol(record: devo_core::CoreSkillRecord) -> SkillRecord {
    SkillRecord {
        id: record.id.0.to_string(),
        name: record.name,
        description: record.description,
        short_description: record.short_description,
        interface: record.interface.map(|interface| ProtocolSkillInterface {
            display_name: interface.display_name,
            short_description: interface.short_description,
            icon_small: interface.icon_small,
            icon_large: interface.icon_large,
            brand_color: interface.brand_color,
            default_prompt: interface.default_prompt,
        }),
        dependencies: record
            .dependencies
            .map(|dependencies| ProtocolSkillDependencies {
                tools: dependencies
                    .tools
                    .into_iter()
                    .map(|tool| ProtocolSkillToolDependency {
                        r#type: tool.r#type,
                        value: tool.value,
                        description: tool.description,
                        transport: tool.transport,
                        command: tool.command,
                        url: tool.url,
                    })
                    .collect(),
            }),
        path: normalize_canonical_path(record.path),
        enabled: record.enabled,
        source: core_skill_source_to_protocol(record.source),
        scope: core_skill_scope_to_protocol(record.scope),
        plugin_id: record.plugin_id,
    }
}

fn core_skill_source_to_protocol(source: devo_core::CoreSkillSource) -> crate::SkillSource {
    match source {
        devo_core::CoreSkillSource::User => crate::SkillSource::User,
        devo_core::CoreSkillSource::Workspace { cwd } => crate::SkillSource::Workspace { cwd },
        devo_core::CoreSkillSource::Plugin { plugin_id } => {
            crate::SkillSource::Plugin { plugin_id }
        }
        devo_core::CoreSkillSource::System => crate::SkillSource::System,
        devo_core::CoreSkillSource::Admin => crate::SkillSource::Admin,
    }
}

fn core_skill_scope_to_protocol(scope: devo_core::CoreSkillScope) -> ProtocolSkillScope {
    match scope {
        devo_core::CoreSkillScope::Repo => ProtocolSkillScope::Repo,
        devo_core::CoreSkillScope::User => ProtocolSkillScope::User,
        devo_core::CoreSkillScope::System => ProtocolSkillScope::System,
        devo_core::CoreSkillScope::Admin => ProtocolSkillScope::Admin,
        devo_core::CoreSkillScope::Plugin => ProtocolSkillScope::Plugin,
    }
}

fn render_resolved_skill(skill: &ResolvedSkill) -> String {
    let base_dir = normalize_canonical_path(
        skill
            .record
            .path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf(),
    );
    format!(
        "<skill id=\"{}\" name=\"{}\">\n{}\n\nBase directory: {}\n</skill>",
        skill.record.id.0,
        skill.record.name,
        skill.content.trim_end(),
        base_dir.display()
    )
}

fn plain_skill_mentions(text: &str) -> HashSet<String> {
    let bytes = text.as_bytes();
    let mut mentions = HashSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let Some(first) = bytes.get(start) else {
            index += 1;
            continue;
        };
        if !is_skill_mention_name_byte(*first) {
            index += 1;
            continue;
        }
        let mut end = start + 1;
        while let Some(next) = bytes.get(end)
            && is_skill_mention_name_byte(*next)
        {
            end += 1;
        }
        let name = &text[start..end];
        if !is_common_env_var(name) {
            mentions.insert(name.to_string());
        }
        index = end;
    }
    mentions
}

fn is_skill_mention_name_byte(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b':')
}

fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(upper.as_str(), "PATH" | "HOME" | "USER" | "SHELL" | "PWD")
}

/// Mutable per-session runtime state owned by the server.
pub(crate) struct RuntimeSession {
    /// Canonical persisted session metadata when the session is durable.
    pub(crate) record: Option<SessionRecord>,
    /// Transport-facing metadata exposed over the API.
    pub(crate) summary: SessionMetadata,
    /// Lock-free snapshot of the session configuration for server coordination paths.
    pub(crate) config: SessionConfig,
    /// Canonical core session state used by the query loop.
    pub(crate) core_session: Arc<Mutex<SessionState>>,
    /// Currently active turn, if any.
    pub(crate) active_turn: Option<TurnMetadata>,
    /// Latest terminal turn metadata for the session.
    pub(crate) latest_turn: Option<TurnMetadata>,
    /// Number of items loaded or appended for the session.
    pub(crate) loaded_item_count: u64,
    /// Replay-friendly ordered history used by interactive clients during session resume.
    pub(crate) history_items: Vec<SessionHistoryItem>,
    /// Canonical persisted turn items in prompt order for replay/compaction bookkeeping.
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    /// Latest compaction snapshot used to rebuild the model-facing prompt view.
    pub(crate) latest_compaction_snapshot: Option<devo_core::CompactionSnapshotLine>,
    /// Shared handle to the pending-turn queue owned by `core_session`.
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    /// Shared handle to the `/btw` queue owned by `core_session`.
    pub(crate) btw_input_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    /// Tool exposure policy for turns run in this session.
    pub(crate) agent_tool_policy: devo_protocol::AgentToolPolicy,
    /// Optional maximum number of turns allowed in this session.
    pub(crate) max_turns: Option<u32>,
    /// Deferred completion info for in-progress assistant text item.
    /// Cleared when the item is completed; used for crash/interrupt recovery.
    pub(crate) deferred_assistant: Option<(devo_core::ItemId, u64, String)>,
    /// Deferred completion info for in-progress reasoning text item.
    pub(crate) deferred_reasoning: Option<(devo_core::ItemId, u64, String)>,
    /// Monotonic session-scoped item sequence counter.
    pub(crate) next_item_seq: u64,
    /// First user input captured from the session's first turn, used for title generation.
    pub(crate) first_user_input: Option<String>,
    /// Active approval requests waiting for client decisions.
    pub(crate) pending_approvals: HashMap<String, PendingApproval>,
    /// Active request_user_input calls waiting for client answers.
    pub(crate) pending_user_inputs: HashMap<String, PendingUserInput>,
    /// Session-scoped approvals granted through approval/respond.
    pub(crate) session_approval_cache: ApprovalGrantCache,
    /// Turn-scoped approvals granted through approval/respond.
    pub(crate) turn_approval_cache: ApprovalGrantCache,
}

impl RuntimeSession {
    /// Wraps a new runtime session in an async mutex for storage in the session map.
    pub(crate) fn shared(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::Model;
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderVendorCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::ProviderWireApi;
    use devo_protocol::StreamEvent;
    use devo_provider::ModelProviderSDK;
    use devo_provider::ProviderRoute;
    use devo_provider::SingleProviderRouter;
    use futures::Stream;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::db::Database;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("not used by turn config resolution tests")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            unreachable!("not used by turn config resolution tests")
        }

        fn name(&self) -> &str {
            "noop"
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("devo-{name}-{nanos}-{id}"))
    }

    fn test_deps(config: &str) -> ServerRuntimeDependencies {
        let root = unique_temp_dir("turn-config-model-name");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("config.toml"), config).expect("write config");
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(Database::open(root.join("test.db")).expect("open db"));

        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            "catalog-slug".to_string(),
            Arc::new(PresetModelCatalog::new(vec![
                Model {
                    slug: "catalog-slug".to_string(),
                    display_name: "Catalog Model".to_string(),
                    ..Model::default()
                },
                Model {
                    slug: "catalog-slug-thinking".to_string(),
                    display_name: "Catalog Thinking Model".to_string(),
                    ..Model::default()
                },
            ])),
            Arc::new(ProviderVendorCatalog::default()),
            None,
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(root, /*workspace_root*/ None).expect("load config"),
            )),
        )
    }

    #[test]
    fn resolve_turn_config_preserves_catalog_slug_and_uses_binding_model_name_for_request() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.other]
enabled = true
name = "Other"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
model_name = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );

        let loaded = deps
            .config_store
            .lock()
            .expect("config store")
            .effective_config()
            .provider
            .providers
            .get("openrouter")
            .and_then(|provider| provider.web_search.as_ref())
            .cloned();
        eprintln!("loaded provider web_search: {loaded:?}");

        let turn_config =
            deps.resolve_turn_config(Some("vendor/model-name"), /*thinking_selection*/ None);

        assert_eq!(turn_config.model.slug, "catalog-slug");
        assert_eq!(turn_config.request_model, "vendor/model-name");
        assert_eq!(
            turn_config.provider_route,
            ProviderRoute::binding("openrouter", ProviderWireApi::OpenAIChatCompletions)
        );
    }

    #[test]
    fn resolve_turn_config_maps_variant_slug_to_binding_model_name() {
        let deps = test_deps(
            r#"
[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
model_name = "vendor/model-name"
invocation_method = "openai_chat_completions"

[model_bindings.thinking]
enabled = true
model_slug = "catalog-slug-thinking"
provider = "openrouter"
model_name = "vendor/model-name-thinking"
invocation_method = "openai_chat_completions"

[model_bindings.other-thinking]
enabled = true
model_slug = "catalog-slug-thinking"
provider = "other"
model_name = "other-provider/model-name-thinking"
invocation_method = "openai_chat_completions"
"#,
        );

        let turn_config = deps.resolve_turn_config(Some("catalog-slug"), None);

        assert_eq!(
            turn_config.provider_request_model("catalog-slug-thinking"),
            "vendor/model-name-thinking"
        );
        assert_eq!(
            turn_config.provider_route,
            ProviderRoute::binding("openrouter", ProviderWireApi::OpenAIChatCompletions)
        );
    }

    #[test]
    fn resolve_turn_config_applies_web_search_provider_override() {
        let deps = test_deps(
            r#"
[tools.web_search]
mode = "disabled"

[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.openrouter.web_search]
mode = "provider"

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
model_name = "vendor/model-name"
invocation_method = "openai_chat_completions"
"#,
        );

        let turn_config =
            deps.resolve_turn_config(Some("vendor/model-name"), /*thinking_selection*/ None);

        assert_eq!(
            turn_config.web_search,
            devo_core::ResolvedWebSearchConfig::Provider
        );
    }

    #[test]
    fn resolve_turn_config_applies_web_search_binding_override() {
        let deps = test_deps(
            r#"
[tools.web_search]
mode = "disabled"

[defaults]
model_binding = "main"

[providers.openrouter]
enabled = true
name = "OpenRouter"
wire_apis = ["openai_chat_completions"]

[providers.openrouter.web_search]
mode = "provider"

[model_bindings.main]
enabled = true
model_slug = "catalog-slug"
provider = "openrouter"
model_name = "vendor/model-name"
invocation_method = "openai_chat_completions"

[model_bindings.main.web_search]
mode = "disabled"
"#,
        );

        let turn_config =
            deps.resolve_turn_config(Some("vendor/model-name"), /*thinking_selection*/ None);

        assert_eq!(
            turn_config.web_search,
            devo_core::ResolvedWebSearchConfig::Disabled
        );
    }
}
