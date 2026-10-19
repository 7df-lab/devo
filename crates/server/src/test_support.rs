//! Shared in-process test helpers.
//!
//! Compiled into the library so both unit tests and `tests/` integration
//! tests can share one `NoopProvider` and one runtime-deps builder.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use devo_core::AgentsMdConfig;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::McpManager;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use futures::stream;

use crate::ProtocolSet;
use crate::ServerRuntime;
use crate::db::Database;
use crate::empty_mcp_manager;
use crate::execution::ServerRuntimeDependencies;
use crate::session_context::SessionRuntimeContext;
use devo_provider::ProviderRouter;

/// Model provider used by most server tests.
///
/// `Fail` bails on every call. `Empty` / `Text` return a finished turn so
/// session-lifecycle tests can start without a real model.
#[derive(Clone, Copy)]
pub struct NoopProvider {
    name: &'static str,
    mode: NoopMode,
}

#[derive(Clone, Copy)]
enum NoopMode {
    Fail,
    Empty,
    Text,
}

impl NoopProvider {
    pub fn failing() -> Self {
        Self {
            name: "noop-provider",
            mode: NoopMode::Fail,
        }
    }

    pub fn empty() -> Self {
        Self {
            name: "noop-provider",
            mode: NoopMode::Empty,
        }
    }

    pub fn text() -> Self {
        Self {
            name: "noop-provider",
            mode: NoopMode::Text,
        }
    }

    pub fn named(self, name: &'static str) -> Self {
        Self { name, ..self }
    }
}

impl Default for NoopProvider {
    fn default() -> Self {
        Self::failing()
    }
}

#[async_trait]
impl ModelProviderSDK for NoopProvider {
    async fn completion(&self, _request: ModelRequest) -> anyhow::Result<ModelResponse> {
        match self.mode {
            NoopMode::Fail => anyhow::bail!("noop provider does not support completion"),
            NoopMode::Empty => Ok(finished_response(Vec::new())),
            NoopMode::Text => Ok(finished_response(vec![ResponseContent::Text(
                "noop".to_string(),
            )])),
        }
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<StreamEvent>> + Send>>,
    > {
        match self.mode {
            NoopMode::Fail => anyhow::bail!("noop provider does not support streaming"),
            NoopMode::Empty | NoopMode::Text => Ok(Box::pin(stream::empty())),
        }
    }

    fn name(&self) -> &str {
        self.name
    }
}

fn finished_response(content: Vec<ResponseContent>) -> ModelResponse {
    ModelResponse {
        id: "noop-response".to_string(),
        content,
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

/// Builder for typical in-process `ServerRuntime` / dependency bundles.
pub struct TestRuntime {
    provider: Arc<dyn ModelProviderSDK>,
    provider_router: Option<Arc<dyn ProviderRouter>>,
    registry: Arc<ToolRegistry>,
    mcp_manager: Arc<dyn McpManager>,
    default_model: String,
    model_catalog: Arc<dyn ModelCatalog>,
    skill_catalog: Option<Box<dyn SkillCatalog + Send>>,
    skills: SkillsConfig,
    agents_md: AgentsMdConfig,
    db_file: String,
    db: Option<Arc<Database>>,
    config_store: Option<Arc<std::sync::Mutex<AppConfigStore>>>,
    protocols: Option<ProtocolSet>,
}

impl TestRuntime {
    pub fn new(provider: Arc<dyn ModelProviderSDK>) -> Self {
        Self {
            provider,
            provider_router: None,
            registry: Arc::new(ToolRegistry::new()),
            mcp_manager: empty_mcp_manager(),
            default_model: "test-model".to_string(),
            model_catalog: Arc::new(PresetModelCatalog::default()),
            skill_catalog: None,
            skills: SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            },
            agents_md: AgentsMdConfig::default(),
            db_file: "test.db".to_string(),
            db: None,
            config_store: None,
            protocols: None,
        }
    }

    pub fn noop() -> Self {
        Self::new(Arc::new(NoopProvider::failing()))
    }

    pub fn noop_text() -> Self {
        Self::new(Arc::new(NoopProvider::text()))
    }

    pub fn noop_empty() -> Self {
        Self::new(Arc::new(NoopProvider::empty()))
    }

    /// Catalog containing a single `test-model` slug, matching most integration tests.
    pub fn with_test_model(self) -> Self {
        self.with_named_model("test-model", "test-model")
    }

    pub fn with_named_model(mut self, slug: &str, display_name: &str) -> Self {
        self.model_catalog = Arc::new(PresetModelCatalog::new(vec![Model {
            slug: slug.to_string(),
            display_name: display_name.to_string(),
            ..Model::default()
        }]));
        self
    }

    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    pub fn catalog(mut self, catalog: Arc<dyn ModelCatalog>) -> Self {
        self.model_catalog = catalog;
        self
    }

    pub fn router(mut self, router: Arc<dyn ProviderRouter>) -> Self {
        self.provider_router = Some(router);
        self
    }

    pub fn database(mut self, db: Arc<Database>) -> Self {
        self.db = Some(db);
        self
    }

    pub fn config_store(mut self, config_store: Arc<std::sync::Mutex<AppConfigStore>>) -> Self {
        self.config_store = Some(config_store);
        self
    }

    pub fn registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.registry = registry;
        self
    }

    pub fn mcp(mut self, mcp_manager: Arc<dyn McpManager>) -> Self {
        self.mcp_manager = mcp_manager;
        self
    }

    pub fn skills(mut self, skills: SkillsConfig) -> Self {
        self.skills = skills;
        self
    }

    pub fn disabled_skills(self) -> Self {
        self.skills(SkillsConfig {
            enabled: false,
            user_roots: Vec::new(),
            workspace_roots: Vec::new(),
            watch_for_changes: false,
            bundled: Some(BundledSkillsConfig { enabled: false }),
            include_instructions: Some(false),
            config: Vec::new(),
        })
    }

    pub fn skill_catalog(mut self, catalog: Box<dyn SkillCatalog + Send>) -> Self {
        self.skill_catalog = Some(catalog);
        self
    }

    pub fn agents_md(mut self, agents_md: AgentsMdConfig) -> Self {
        self.agents_md = agents_md;
        self
    }

    pub fn db_file(mut self, name: impl Into<String>) -> Self {
        self.db_file = name.into();
        self
    }

    pub fn protocols(mut self, protocols: ProtocolSet) -> Self {
        self.protocols = Some(protocols);
        self
    }

    pub fn deps(self, data_root: &Path) -> ServerRuntimeDependencies {
        let db = self.db.unwrap_or_else(|| {
            Arc::new(Database::open(data_root.join(&self.db_file)).expect("open test database"))
        });
        let config_store = self.config_store.unwrap_or_else(|| {
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(data_root.to_path_buf(), None).expect("load app config store"),
            ))
        });
        let skill_catalog = self
            .skill_catalog
            .unwrap_or_else(|| Box::new(FileSystemSkillCatalog::new(self.skills)));
        let provider_router = self
            .provider_router
            .unwrap_or_else(|| Arc::new(SingleProviderRouter::new(Arc::clone(&self.provider))));
        let process_context = Arc::new(SessionRuntimeContext::from_parts(
            self.provider,
            provider_router,
            self.registry,
            self.mcp_manager,
            self.default_model,
            self.model_catalog,
            Arc::new(std::sync::Mutex::new(skill_catalog)),
            self.agents_md,
            config_store,
        ));
        ServerRuntimeDependencies::new(process_context, db)
    }

    pub fn runtime(mut self, data_root: &Path) -> Arc<ServerRuntime> {
        let protocols = self.protocols.take();
        let home = data_root.to_path_buf();
        let deps = self.deps(data_root);
        match protocols {
            Some(protocols) => ServerRuntime::with_protocols(home, deps, protocols),
            None => ServerRuntime::new(home, deps),
        }
    }
}

/// Shared test helper used by unit and integration tests.
pub fn test_dependencies(data_root: &Path) -> ServerRuntimeDependencies {
    TestRuntime::noop().deps(data_root)
}

/// Shared test helper with a custom catalog and default model slug.
pub fn test_dependencies_with_catalog(
    data_root: &Path,
    default_model: impl Into<String>,
    model_catalog: Arc<dyn ModelCatalog>,
) -> ServerRuntimeDependencies {
    TestRuntime::noop()
        .default_model(default_model)
        .catalog(model_catalog)
        .deps(data_root)
}

impl ServerRuntimeDependencies {
    /// Shared test helper: noop provider, empty registry/MCP, default catalog.
    pub fn test_dependencies(data_root: &Path) -> Self {
        TestRuntime::noop().deps(data_root)
    }

    /// Shared test helper with a custom catalog and default model slug.
    pub fn test_dependencies_with_catalog(
        data_root: &Path,
        default_model: impl Into<String>,
        model_catalog: Arc<dyn ModelCatalog>,
    ) -> Self {
        TestRuntime::noop()
            .default_model(default_model)
            .catalog(model_catalog)
            .deps(data_root)
    }

    /// Shared test helper with a custom model provider.
    pub fn test_dependencies_with_provider(
        data_root: &Path,
        provider: Arc<dyn ModelProviderSDK>,
    ) -> Self {
        TestRuntime::new(provider).deps(data_root)
    }
}
