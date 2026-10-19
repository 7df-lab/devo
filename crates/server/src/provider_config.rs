//! Server-side provider bootstrap and routing.
//!
//! This module keeps provider construction at the runtime boundary: config and
//! auth are resolved once into concrete provider adapters, while later turns
//! select between those adapters through a route-aware facade.

use anyhow::Context;
use anyhow::Result;

use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::AppConfig;
use devo_core::AuthCredentialKind;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderConfigEntry;
use devo_core::ProviderConfigFile;
use devo_core::ProviderHttpConfig;
use devo_core::ProviderWireApi;
use devo_core::UserAuthConfigFile;
use devo_core::default_provider_credential_id;
use devo_core::read_user_auth_config;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;
use devo_provider::MultiProviderRouter;
use devo_provider::ProviderHttpOptions;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use devo_provider::SingleProviderRouter;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::google::{DEFAULT_GOOGLE_GENERATIVE_AI_BASE_URL, GoogleGenerativeAiProvider};
use devo_provider::openai::OpenAIProvider;
use devo_provider::openai::OpenAIResponsesProvider;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

const NO_PROVIDER_CONFIGURED_MESSAGE: &str =
    "No provider configured. Run `devo onboard` to complete setup.";

/// Resolved provider bootstrap owned by the server runtime.
pub struct ResolvedServerProvider {
    /// Concrete provider used for model requests.
    pub provider: Arc<dyn ModelProviderSDK>,
    /// Route-aware provider facade used for model requests.
    pub provider_router: Arc<dyn ProviderRouter>,
    /// Default model slug used when a session or turn does not request one.
    pub default_model: String,
}

/// Loads the server-side provider from a merged app config.
pub async fn load_server_provider(
    app_config: &AppConfig,
    default_model: Option<&str>,
    user_config_dir: &Path,
) -> Result<ResolvedServerProvider> {
    if !app_config.has_provider_configuration() {
        let default_model = match default_model {
            Some(default_model) => default_model.to_string(),
            None => PresetModelCatalog::load()?
                .resolve_for_turn(None)?
                .slug
                .clone(),
        };
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(MissingProvider);
        return Ok(ResolvedServerProvider {
            provider: Arc::clone(&provider),
            provider_router: Arc::new(SingleProviderRouter::new(provider)),
            default_model,
        });
    }

    let provider_config = app_config.provider_catalog_config();
    let auth = read_user_auth_config(&user_config_dir.join(AUTH_CONFIG_FILE_NAME))?;
    let selection =
        resolve_server_model_with_home(&provider_config, default_model, Some(user_config_dir))?;
    let effective =
        devo_core::effective_provider_catalog_with_home(&provider_config, Some(user_config_dir))
            .context("failed to merge builtin provider catalog with user overlay")?;
    let provider_config_entry = effective
        .providers
        .get(&selection.provider_id)
        .with_context(|| {
            format!(
                "configured provider Connection `{}` was not found",
                selection.provider_id
            )
        })?;
    let provider = match build_provider_route(
        selection.wire_api,
        &selection.provider_id,
        provider_config_entry,
        &auth,
        &app_config.provider_http,
        user_config_dir,
    )
    .await
    {
        Ok(provider) => provider,
        Err(error) => Arc::new(UnavailableProvider::new(error.to_string())),
    };
    let auth = read_user_auth_config(&user_config_dir.join(AUTH_CONFIG_FILE_NAME))?;
    // Route table stays sparse/user Connections only — do not register every builtin.
    // Adapter fields (base_url, wire_api, headers) come from the effective catalog
    // so sparse Connections still inherit builtin endpoints.
    let provider_router = build_multi_provider_router(
        &provider_config,
        &effective,
        &app_config.provider_http,
        &auth,
        Arc::clone(&provider),
        user_config_dir,
    )
    .await?;
    Ok(ResolvedServerProvider {
        provider,
        provider_router,
        default_model: format!("{}/{}", selection.provider_id, selection.model_id),
    })
}

struct MissingProvider;

#[async_trait::async_trait]
impl ModelProviderSDK for MissingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!(NO_PROVIDER_CONFIGURED_MESSAGE)
    }

    fn name(&self) -> &str {
        "missing-provider"
    }
}

struct UnavailableProvider {
    message: String,
}

impl UnavailableProvider {
    fn new(message: String) -> Self {
        Self { message }
    }
}

#[async_trait::async_trait]
impl ModelProviderSDK for UnavailableProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("{}", self.message)
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("{}", self.message)
    }

    fn name(&self) -> &str {
        "unavailable-provider"
    }
}

pub(crate) fn build_provider_adapter(
    wire_api: ProviderWireApi,
    base_url: Option<String>,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
    auth_kind: Option<AuthCredentialKind>,
) -> Result<Arc<dyn ModelProviderSDK>> {
    let provider: Arc<dyn ModelProviderSDK> = match wire_api {
        ProviderWireApi::AnthropicMessages => {
            let api_key =
                api_key.context("anthropic provider requires an API key or OAuth access token")?;
            let base_url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
            let mut anthropic = AnthropicProvider::new(base_url).with_http_options(http_options)?;
            anthropic = match auth_kind {
                Some(AuthCredentialKind::Oauth) => anthropic.with_oauth_access(api_key),
                _ => anthropic.with_api_key(api_key),
            };
            Arc::new(anthropic)
        }
        ProviderWireApi::OpenAIChatCompletions => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider = OpenAIProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Arc::new(provider)
        }
        ProviderWireApi::OpenAIResponses => {
            let base_url = normalize_openai_base_url(
                &base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
            );
            let mut provider =
                OpenAIResponsesProvider::new(base_url).with_http_options(http_options)?;
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            Arc::new(provider)
        }
        ProviderWireApi::GoogleGenerativeAi => {
            let api_key = api_key.context("Google Generative AI provider requires an API key")?;
            let base_url =
                base_url.unwrap_or_else(|| DEFAULT_GOOGLE_GENERATIVE_AI_BASE_URL.to_string());
            Arc::new(
                GoogleGenerativeAiProvider::new(base_url, api_key)
                    .with_http_options(http_options)?,
            )
        }
    };

    Ok(provider)
}

async fn build_multi_provider_router(
    sparse_connections: &ProviderConfigFile,
    effective_catalog: &ProviderConfigFile,
    provider_http: &ProviderHttpConfig,
    auth: &UserAuthConfigFile,
    default_provider: Arc<dyn ModelProviderSDK>,
    user_config_dir: &Path,
) -> Result<Arc<dyn ProviderRouter>> {
    let mut router = MultiProviderRouter::new(default_provider);
    for (route, provider) in build_connection_route_table(
        sparse_connections,
        effective_catalog,
        provider_http,
        auth,
        user_config_dir,
    )
    .await?
    {
        router.insert_route(route, provider);
    }
    Ok(Arc::new(router))
}

/// Builds the Connection-route table (`provider_id` × wire API → adapter)
/// from a merged provider config. Shared by server bootstrap and live
/// provider reloads so both construct identical routes.
pub(crate) async fn build_connection_route_table(
    sparse_connections: &ProviderConfigFile,
    effective_catalog: &ProviderConfigFile,
    provider_http: &ProviderHttpConfig,
    auth: &UserAuthConfigFile,
    user_config_dir: &Path,
) -> Result<Vec<(ProviderRoute, Arc<dyn ModelProviderSDK>)>> {
    let mut routes = Vec::new();
    for (provider_id, sparse_provider) in &sparse_connections.providers {
        if sparse_provider.enabled == Some(false) {
            continue;
        }
        // Sparse overlays omit unchanged builtin fields (including base_url).
        // Build the HTTP adapter from the effective entry so openai-compatible
        // Connections do not fall back to api.openai.com.
        let provider =
            connection_provider_entry_for_adapter(provider_id, sparse_provider, effective_catalog);
        let mut wire_apis = Vec::new();
        if let Some(wire_api) = provider.wire_api {
            wire_apis.push(wire_api);
        }
        for model in provider.models.values() {
            if let Some(wire_api) = model.wire_api
                && !wire_apis.contains(&wire_api)
            {
                wire_apis.push(wire_api);
            }
        }
        if wire_apis.is_empty() {
            wire_apis.push(ProviderWireApi::OpenAIChatCompletions);
        }
        let api_key =
            resolve_provider_api_key(provider_id, sparse_provider, auth, user_config_dir).await;
        let credential_id = resolve_provider_credential_id(provider_id, sparse_provider, auth);
        let auth_kind = credential_id
            .as_deref()
            .and_then(|id| auth.credentials.get(id))
            .map(|credential| credential.kind);
        let account_id = credential_id.as_deref().and_then(|id| {
            auth.credentials
                .get(id)
                .and_then(|credential| credential.account_id.clone())
        });
        let mut headers = provider.headers.clone().unwrap_or_default();
        if provider_id == "openai-codex"
            && let Some(account_id) = account_id.filter(|value| !value.is_empty())
        {
            headers
                .entry("chatgpt-account-id".to_string())
                .or_insert(account_id);
        }
        for wire_api in wire_apis {
            let provider_instance = match &api_key {
                Ok(api_key) => build_provider_adapter(
                    wire_api,
                    provider.base_url.clone(),
                    api_key.clone(),
                    ProviderHttpOptions::from_raw_with_no_proxy(
                        provider_http.proxy_url.clone(),
                        provider_http.no_proxy.clone(),
                        if headers.is_empty() {
                            None
                        } else {
                            Some(serde_json::to_string(&headers)?)
                        },
                    )?,
                    auth_kind,
                )
                .unwrap_or_else(|error| Arc::new(UnavailableProvider::new(error.to_string()))),
                Err(error) => Arc::new(UnavailableProvider::new(error.to_string())),
            };
            routes.push((
                ProviderRoute::connection(provider_id.clone(), wire_api),
                provider_instance,
            ));
        }
    }

    Ok(routes)
}

/// Picks the provider entry used to construct a Connection HTTP adapter.
///
/// Prefer the effective (builtin + overlay) entry so sparse `providers.json`
/// Connections inherit `base_url` / `wire_api` from the embedded directory.
pub(crate) fn connection_provider_entry_for_adapter<'a>(
    provider_id: &str,
    sparse: &'a ProviderConfigEntry,
    effective: &'a ProviderConfigFile,
) -> &'a ProviderConfigEntry {
    effective.providers.get(provider_id).unwrap_or(sparse)
}

async fn build_provider_route(
    wire_api: ProviderWireApi,
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &UserAuthConfigFile,
    provider_http: &ProviderHttpConfig,
    user_config_dir: &Path,
) -> Result<Arc<dyn ModelProviderSDK>> {
    let credential_id = resolve_provider_credential_id(provider_id, provider, auth);
    let auth_kind = credential_id
        .as_deref()
        .and_then(|id| auth.credentials.get(id))
        .map(|credential| credential.kind);
    let account_id = credential_id.as_deref().and_then(|id| {
        auth.credentials
            .get(id)
            .and_then(|credential| credential.account_id.clone())
    });
    let mut headers = provider.headers.clone().unwrap_or_default();
    if provider_id == "openai-codex"
        && let Some(account_id) = account_id.filter(|value| !value.is_empty())
    {
        headers
            .entry("chatgpt-account-id".to_string())
            .or_insert(account_id);
    }
    build_provider_adapter(
        wire_api,
        provider.base_url.clone(),
        resolve_provider_api_key(provider_id, provider, auth, user_config_dir).await?,
        ProviderHttpOptions::from_raw_with_no_proxy(
            provider_http.proxy_url.clone(),
            provider_http.no_proxy.clone(),
            if headers.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&headers)?)
            },
        )?,
        auth_kind,
    )
}

fn resolve_server_model_with_home(
    provider_config: &ProviderConfigFile,
    default_model: Option<&str>,
    home_dir: Option<&Path>,
) -> Result<devo_core::ProviderModelSelection> {
    // Sparse providers.json overlays omit unchanged builtin models; resolve
    // against the effective (builtin + remote cache + overlay) catalog.
    let effective = devo_core::effective_provider_catalog_with_home(provider_config, home_dir)
        .context("failed to merge builtin provider catalog with user overlay")?;
    // Explicit caller override (CLI `--model`, session default argument) wins
    // over a persisted catalog/config.toml selection.
    if let Some(default_model) = default_model
        && let Ok(selection) = effective.resolve_model(Some(default_model))
    {
        return Ok(selection);
    }
    effective.resolve_model(None).map_err(Into::into)
}

pub(crate) async fn resolve_provider_api_key(
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &devo_core::UserAuthConfigFile,
    user_config_dir: &Path,
) -> Result<Option<String>> {
    let Some(credential_id) = resolve_provider_credential_id(provider_id, provider, auth) else {
        return Ok(None);
    };
    let credential = auth.credentials.get(&credential_id).with_context(|| {
        format!(
            "provider `{provider_id}` references missing credential `{credential_id}` in user auth.json"
        )
    })?;
    match credential.kind {
        AuthCredentialKind::ApiKey => {
            if credential.value.is_empty() {
                anyhow::bail!("credential `{credential_id}` has an empty API key value");
            }
            Ok(Some(credential.value.clone()))
        }
        AuthCredentialKind::Oauth => crate::oauth_refresh::resolve_oauth_access(
            provider_id,
            &credential_id,
            credential,
            user_config_dir,
        )
        .await
        .map(Some),
    }
}

/// Prefer an explicit provider `credential` binding; otherwise discover the
/// Provider-keyed auth.json entry, with legacy `{provider}_oauth` /
/// `{provider}_api_key` fallbacks for unmigrated files.
pub(crate) fn resolve_provider_credential_id(
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &devo_core::UserAuthConfigFile,
) -> Option<String> {
    if let Some(credential_id) = provider
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if auth.credentials.contains_key(credential_id) {
            return Some(credential_id.to_string());
        }
        let migrated = devo_core::provider_id_from_credential_id(credential_id);
        if auth.credentials.contains_key(&migrated) {
            return Some(migrated);
        }
    }
    if auth.credentials.contains_key(provider_id) {
        return Some(provider_id.to_string());
    }
    let provider_key = default_provider_credential_id(provider_id);
    if auth.credentials.contains_key(&provider_key) {
        return Some(provider_key);
    }
    let oauth_id = format!("{}_oauth", provider_id.replace(['/', '\\'], "_"));
    if auth.credentials.contains_key(&oauth_id) {
        return Some(oauth_id);
    }
    let api_key_id = devo_core::legacy_provider_api_key_credential_id(provider_id);
    if auth.credentials.contains_key(&api_key_id) {
        return Some(api_key_id);
    }
    None
}

pub(crate) fn normalize_openai_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let Some(scheme_sep) = trimmed.find("://") else {
        return trimmed.to_string();
    };
    let has_explicit_path = trimmed[scheme_sep + 3..].contains('/');
    if has_explicit_path {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_core::AppConfig;
    use devo_core::AuthCredentialConfig;
    use devo_core::AuthCredentialKind;
    use devo_core::ProviderConfigEntry;
    use devo_core::ProviderConfigFile;
    use devo_core::ProviderModelConfig;
    use devo_core::UserAuthConfigFile;
    use pretty_assertions::assert_eq;

    use super::load_server_provider;
    use super::normalize_openai_base_url;
    use super::resolve_provider_api_key;
    use devo_protocol::ProviderWireApi;

    #[test]
    fn preserves_explicit_openai_compatible_paths() {
        assert_eq!(
            normalize_openai_base_url("https://open.bigmodel.cn/api/paas/v4/"),
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }

    #[test]
    fn appends_v1_for_bare_openai_hosts() {
        assert_eq!(
            normalize_openai_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
    }

    /// Trace: sparse Connection overlays omit builtin base_url; adapters must
    /// still inherit https://api.deepseek.com instead of defaulting to OpenAI.
    #[test]
    fn sparse_deepseek_connection_inherits_builtin_base_url_for_adapter() {
        let sparse = ProviderConfigEntry {
            name: Some("deepseek".to_string()),
            credential: Some("deepseek".to_string()),
            enabled: Some(true),
            ..ProviderConfigEntry::default()
        };
        let overlay = ProviderConfigFile {
            providers: BTreeMap::from([("deepseek".to_string(), sparse.clone())]),
            ..ProviderConfigFile::default()
        };
        let effective = devo_core::effective_provider_catalog(&overlay)
            .expect("merge builtin deepseek catalog");
        let entry = super::connection_provider_entry_for_adapter("deepseek", &sparse, &effective);
        assert_eq!(
            entry.base_url.as_deref(),
            Some("https://api.deepseek.com"),
            "sparse Connection must inherit builtin DeepSeek base_url"
        );
        assert_eq!(
            normalize_openai_base_url(entry.base_url.as_deref().unwrap()),
            "https://api.deepseek.com/v1"
        );
        assert_ne!(
            normalize_openai_base_url(
                &sparse
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com".to_string())
            ),
            "https://api.deepseek.com/v1",
            "precondition: sparse-only path would incorrectly hit OpenAI"
        );
    }

    #[tokio::test]
    async fn empty_provider_config_loads_missing_provider_for_onboarding() {
        let config = devo_core::AppConfig::default();
        let dir = tempfile::tempdir().expect("temp dir");

        let actual = load_server_provider(&config, Some("onboard-model"), dir.path())
            .await
            .expect("load missing provider");

        assert_eq!(actual.default_model, "onboard-model");
        assert_eq!(actual.provider.name(), "missing-provider");
        let error = actual
            .provider
            .completion(devo_protocol::ModelRequest {
                model_slug: devo_protocol::ModelProfileKey::Generic,
                model: "onboard-model".to_string(),
                system: None,
                messages: Vec::new(),
                max_tokens: 1,
                tools: None,
                hosted_tools: Vec::new(),
                sampling: devo_protocol::SamplingControls::default(),
                request_thinking: None,
                reasoning_effort: None,
                extra_body: None,
            })
            .await
            .expect_err("missing provider should reject model requests");

        assert_eq!(
            error.to_string(),
            "No provider configured. Run `devo onboard` to complete setup."
        );
    }

    /// Trace: L2-DES-APP-005, L2-DES-MODEL-001
    /// Verifies: server provider construction validates provider custom header configuration.
    #[tokio::test]
    async fn load_server_provider_rejects_invalid_custom_headers() {
        let config = AppConfig {
            provider_catalog: ProviderConfigFile {
                model: Some("openai/test-model".to_string()),
                providers: BTreeMap::from([(
                    "openai".to_string(),
                    ProviderConfigEntry {
                        name: Some("OpenAI".to_string()),
                        headers: Some(BTreeMap::from([(
                            "bad header".to_string(),
                            "value".to_string(),
                        )])),
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        models: BTreeMap::from([(
                            "test-model".to_string(),
                            ProviderModelConfig::default(),
                        )]),
                        ..ProviderConfigEntry::default()
                    },
                )]),
                ..ProviderConfigFile::default()
            },
            ..AppConfig::default()
        };
        let dir = tempfile::tempdir().expect("temp dir");

        let error = match load_server_provider(&config, None, dir.path()).await {
            Ok(_) => panic!("invalid headers should reject provider construction"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "invalid provider custom header name `bad header`"
        );
    }

    #[tokio::test]
    async fn resolves_provider_credential_id_through_user_auth() {
        let provider = ProviderConfigEntry {
            credential: Some("openrouter_api_key".to_string()),
            ..ProviderConfigEntry::default()
        };
        let auth = UserAuthConfigFile {
            credentials: BTreeMap::from([(
                "openrouter_api_key".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::ApiKey,
                    value: "sk-or-secret".to_string(),
                    access: None,
                    refresh: None,
                    expires_at: None,
                    account_id: None,
                    enterprise_url: None,
                },
            )]),
            ..UserAuthConfigFile::default()
        };

        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            resolve_provider_api_key("openrouter", &provider, &auth, dir.path())
                .await
                .expect("resolve provider credential"),
            Some("sk-or-secret".to_string())
        );
    }

    /// Trace: L2-DES-AUTH-001
    /// Verifies: oauth credentials are discoverable for builtins without an explicit providers.json binding.
    #[test]
    fn resolve_provider_credential_id_discovers_oauth_convention() {
        let provider = ProviderConfigEntry::default();
        let auth = UserAuthConfigFile {
            credentials: BTreeMap::from([(
                "anthropic_oauth".to_string(),
                AuthCredentialConfig {
                    kind: AuthCredentialKind::Oauth,
                    value: String::new(),
                    access: Some("access".to_string()),
                    refresh: Some("refresh".to_string()),
                    expires_at: None,
                    account_id: None,
                    enterprise_url: None,
                },
            )]),
            ..UserAuthConfigFile::default()
        };
        assert_eq!(
            super::resolve_provider_credential_id("anthropic", &provider, &auth).as_deref(),
            Some("anthropic_oauth")
        );
    }
}
