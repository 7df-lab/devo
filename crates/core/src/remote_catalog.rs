//! Live models.dev discovery. Successful results live only in this process.
//! Neither loading nor refreshing reads or writes a persisted catalog snapshot.

use crate::{
    CatalogConfig, InputModality, ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig,
    ProviderWireApi, ReasoningCapability, ReasoningEffort, levels_with_leading_off,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/api.json";
const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRefreshOutcome {
    Updated {
        providers: usize,
        models: usize,
    },
    CacheFresh,
    SkippedOffline,
    SkippedStartupDisabled,
    Failed {
        stage: CatalogRefreshStage,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRefreshStage {
    ReadSource,
    HttpRequest,
    HttpStatus,
    Parse,
    Convert,
}

#[derive(Clone)]
struct LiveCatalog {
    source: String,
    fetched_at: Instant,
    catalog: ProviderConfigFile,
}
static LIVE_CATALOGS: OnceLock<Mutex<BTreeMap<PathBuf, LiveCatalog>>> = OnceLock::new();
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Returns the last successful catalog for this home, without disk access.
pub fn load_remote_catalog_overlay(home_dir: &Path) -> anyhow::Result<Option<ProviderConfigFile>> {
    let catalogs = LIVE_CATALOGS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("live catalog lock poisoned"))?;
    Ok(catalogs.get(home_dir).map(|live| live.catalog.clone()))
}

/// Refresh at startup or when opening discovery, respecting freshness settings.
pub async fn refresh_remote_catalog(
    home_dir: &Path,
    config: &CatalogConfig,
) -> CatalogRefreshOutcome {
    if !config.refresh_on_startup {
        return CatalogRefreshOutcome::SkippedStartupDisabled;
    }
    refresh_catalog(home_dir, config, RefreshMode::IfStale).await
}

/// Explicit refresh ignores the interval and startup flag, but honors offline mode.
pub async fn refresh_remote_catalog_force(
    home_dir: &Path,
    config: &CatalogConfig,
) -> CatalogRefreshOutcome {
    refresh_catalog(home_dir, config, RefreshMode::Force).await
}

enum RefreshMode {
    IfStale,
    Force,
}

async fn refresh_catalog(
    home_dir: &Path,
    config: &CatalogConfig,
    mode: RefreshMode,
) -> CatalogRefreshOutcome {
    let _refresh_guard = REFRESH_LOCK.lock().await;
    let source = config.source.trim();
    let source = if source.is_empty() {
        DEFAULT_MODELS_DEV_URL
    } else {
        source
    };
    let is_url = source.starts_with("http://") || source.starts_with("https://");
    if is_url && config.offline {
        return CatalogRefreshOutcome::SkippedOffline;
    }
    if matches!(mode, RefreshMode::IfStale) {
        let catalogs = LIVE_CATALOGS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if catalogs.get(home_dir).is_some_and(|live| {
            live.source == source
                && live.fetched_at.elapsed()
                    < Duration::from_secs(config.refresh_interval_hours.saturating_mul(3600))
        }) {
            return CatalogRefreshOutcome::CacheFresh;
        }
    }
    let raw = if is_url {
        match download_models_dev(source).await {
            Ok(raw) => raw,
            Err(outcome) => return outcome,
        }
    } else {
        // An explicitly configured local fixture is supported; never search cache paths.
        match tokio::fs::File::open(source).await {
            Ok(file) => {
                use tokio::io::AsyncReadExt;
                let mut raw = Vec::new();
                match file
                    .take((MAX_CATALOG_BYTES + 1) as u64)
                    .read_to_end(&mut raw)
                    .await
                {
                    Ok(_) if raw.len() <= MAX_CATALOG_BYTES => raw,
                    Ok(_) => {
                        return failure(
                            CatalogRefreshStage::ReadSource,
                            "catalog exceeds size limit",
                        );
                    }
                    Err(error) => return failure(CatalogRefreshStage::ReadSource, error),
                }
            }
            Err(error) => return failure(CatalogRefreshStage::ReadSource, error),
        }
    };
    let api: Value = match serde_json::from_slice(&raw) {
        Ok(api) => api,
        Err(error) => return failure(CatalogRefreshStage::Parse, error),
    };
    let (catalog, providers, models) = match convert_models_dev_api(&api) {
        Ok(result) => result,
        Err(error) => return failure(CatalogRefreshStage::Convert, error),
    };
    if providers == 0 || models == 0 {
        return failure(
            CatalogRefreshStage::Convert,
            "catalog contains no supported models",
        );
    }
    LIVE_CATALOGS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            home_dir.to_path_buf(),
            LiveCatalog {
                source: source.to_string(),
                fetched_at: Instant::now(),
                catalog,
            },
        );
    CatalogRefreshOutcome::Updated { providers, models }
}

fn failure(stage: CatalogRefreshStage, message: impl std::fmt::Display) -> CatalogRefreshOutcome {
    CatalogRefreshOutcome::Failed {
        stage,
        message: message.to_string(),
    }
}

async fn download_models_dev(url: &str) -> Result<Vec<u8>, CatalogRefreshOutcome> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent(format!("devo/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| failure(CatalogRefreshStage::HttpRequest, error))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| failure(CatalogRefreshStage::HttpRequest, error))?;
    if !response.status().is_success() {
        return Err(failure(
            CatalogRefreshStage::HttpStatus,
            format!("models.dev returned HTTP {}", response.status()),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
    {
        return Err(failure(
            CatalogRefreshStage::HttpRequest,
            "catalog exceeds size limit",
        ));
    }
    let mut raw = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| failure(CatalogRefreshStage::HttpRequest, error))?
    {
        if raw.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(failure(
                CatalogRefreshStage::HttpRequest,
                "catalog exceeds size limit",
            ));
        }
        raw.extend_from_slice(&chunk);
    }
    Ok(raw)
}

/// Adapter capabilities, not provider identities, determine supported protocols.
fn wire_api_for_npm(npm: &str) -> Option<ProviderWireApi> {
    match npm {
        "@ai-sdk/openai" => Some(ProviderWireApi::OpenAIResponses),
        "@ai-sdk/anthropic" => Some(ProviderWireApi::AnthropicMessages),
        "@ai-sdk/google" => Some(ProviderWireApi::GoogleGenerativeAi),
        "@ai-sdk/openai-compatible"
        | "@openrouter/ai-sdk-provider"
        | "@ai-sdk/deepseek"
        | "@ai-sdk/xai"
        | "@ai-sdk/groq"
        | "@ai-sdk/togetherai"
        | "@ai-sdk/fireworks"
        | "@ai-sdk/perplexity" => Some(ProviderWireApi::OpenAIChatCompletions),
        _ => None,
    }
}

/// SDK transport defaults are adapter behavior, not a provider directory.
fn default_endpoint_for_npm(npm: &str) -> Option<&'static str> {
    match npm {
        "@ai-sdk/openai" => Some("https://api.openai.com/v1"),
        "@ai-sdk/anthropic" => Some("https://api.anthropic.com"),
        "@ai-sdk/google" => Some("https://generativelanguage.googleapis.com/v1beta"),
        _ => None,
    }
}

/// Converts every supported provider in models.dev, including previously unknown ids.
pub fn convert_models_dev_api(api: &Value) -> Result<(ProviderConfigFile, usize, usize), String> {
    let root = api
        .as_object()
        .ok_or_else(|| "models.dev api.json root must be an object".to_string())?;
    let mut catalog = ProviderConfigFile::default();
    let mut model_count = 0;
    for (provider_id, remote) in root {
        let Some(models) = remote.get("models").and_then(Value::as_object) else {
            continue;
        };
        let npm = remote
            .get("npm")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let protocol = wire_api_for_npm(npm);
        let mut entry = ProviderConfigEntry {
            name: remote
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            base_url: remote
                .get("api")
                .and_then(Value::as_str)
                .or_else(|| default_endpoint_for_npm(npm))
                .map(str::to_owned),
            wire_api: protocol,
            env: remote
                .get("env")
                .and_then(Value::as_array)
                .map(|env| {
                    env.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            ..ProviderConfigEntry::default()
        };
        for (model_id, remote_model) in models {
            if remote_model.get("tool_call") == Some(&Value::Bool(false))
                || remote_model.get("status").and_then(Value::as_str) == Some("deprecated")
            {
                continue;
            }
            let model_npm = remote_model
                .pointer("/provider/npm")
                .and_then(Value::as_str);
            let Some(model_protocol) = model_npm.map(wire_api_for_npm).unwrap_or(protocol) else {
                continue;
            };
            let mut model = model_from_models_dev(remote_model);
            model.wire_api = Some(model_protocol);
            model.base_url = remote_model
                .pointer("/provider/api")
                .and_then(Value::as_str)
                .or_else(|| {
                    // An explicit provider endpoint remains authoritative when a
                    // model changes only its SDK. Otherwise use that SDK's default.
                    if remote.get("api").and_then(Value::as_str).is_none() {
                        model_npm.and_then(default_endpoint_for_npm)
                    } else {
                        None
                    }
                })
                .map(str::to_owned);
            model.request_model = remote_model
                .pointer("/provider/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            entry.models.insert(model_id.clone(), model);
            model_count += 1;
        }
        if !entry.models.is_empty() {
            catalog.providers.insert(provider_id.clone(), entry);
        }
    }
    let provider_count = catalog.providers.len();
    Ok((catalog, provider_count, model_count))
}

fn model_from_models_dev(remote: &Value) -> ProviderModelConfig {
    let mut model = ProviderModelConfig::default();
    model.origin = Some(devo_protocol::ProviderModelOrigin::Remote);

    if let Some(name) = remote.get("name").and_then(Value::as_str) {
        model.name = Some(name.to_string());
    }
    if let Some(family) = remote.get("family").and_then(Value::as_str) {
        model.family = Some(family.to_string());
    }
    if let Some(release_date) = remote.get("release_date").and_then(Value::as_str) {
        model.release_date = Some(release_date.to_string());
    }
    if let Some(status) = remote.get("status").and_then(Value::as_str) {
        model.status = Some(status.to_string());
    }
    if let Some(context) = remote
        .pointer("/limit/context")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    {
        model.context_window = Some(context);
    }
    if let Some(output) = remote
        .pointer("/limit/output")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    {
        model.max_tokens = Some(output);
    }

    let reasoning = remote.get("reasoning").and_then(Value::as_bool) == Some(true);
    if reasoning {
        model.reasoning = Some(true);
        if matches!(
            model.reasoning_capability,
            None | Some(ReasoningCapability::Unsupported)
        ) {
            model.reasoning_capability =
                Some(ReasoningCapability::Levels(levels_with_leading_off([
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ])));
            model.default_reasoning_effort = Some(ReasoningEffort::High);
        }
    }

    if let Some(inputs) = remote
        .pointer("/modalities/input")
        .and_then(Value::as_array)
    {
        let mut modalities = Vec::new();
        for input in inputs {
            match input.as_str() {
                Some("text") => modalities.push(InputModality::Text),
                Some("image") => modalities.push(InputModality::Image),
                _ => {}
            }
        }
        if !modalities.is_empty() {
            model.input_modalities = Some(modalities);
        }
    }

    if let Some(cost) = remote.get("cost") {
        model.cost = Some(serde_json::json!({
            "input": cost.get("input").cloned().unwrap_or(Value::from(0)),
            "output": cost.get("output").cloned().unwrap_or(Value::from(0)),
            "cacheRead": cost.get("cache_read").cloned().unwrap_or(Value::from(0)),
            "cacheWrite": cost.get("cache_write").cloned().unwrap_or(Value::from(0)),
        }));
    }

    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn fixture() -> Value {
        json!({"new-provider": {"npm": "@ai-sdk/openai-compatible", "api": "https://api.example/v1", "models": {
            "model": {"tool_call": true, "name": "Remote Model", "limit": {"context": 64000}},
            "special": {"provider": {"npm": "@ai-sdk/anthropic", "api": "https://messages.example", "id": "wire-id"}},
            "unsupported": {"provider": {"npm": "@vendor/unknown"}},
            "embedding": {"tool_call": false},
            "old": {"status": "deprecated"}
        }}})
    }

    #[test]
    fn discovers_unknown_provider_and_model_adapter_overrides() {
        let (catalog, providers, models) = convert_models_dev_api(&fixture()).unwrap();
        assert_eq!((providers, models), (1, 2));
        let entry = &catalog.providers["new-provider"];
        assert_eq!(entry.wire_api, Some(ProviderWireApi::OpenAIChatCompletions));
        let special = &entry.models["special"];
        assert_eq!(
            (
                special.wire_api,
                special.base_url.as_deref(),
                special.request_model.as_deref()
            ),
            (
                Some(ProviderWireApi::AnthropicMessages),
                Some("https://messages.example"),
                Some("wire-id")
            )
        );
    }

    #[tokio::test]
    async fn refresh_is_memory_only_and_failures_keep_last_good_data() {
        let home = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), fixture().to_string()).unwrap();
        let config = CatalogConfig {
            source: source.path().to_string_lossy().into_owned(),
            ..CatalogConfig::default()
        };
        assert_eq!(
            refresh_remote_catalog(home.path(), &config).await,
            CatalogRefreshOutcome::Updated {
                providers: 1,
                models: 2
            }
        );
        let before = load_remote_catalog_overlay(home.path()).unwrap();
        assert_eq!(
            refresh_remote_catalog(home.path(), &config).await,
            CatalogRefreshOutcome::CacheFresh
        );
        std::fs::write(source.path(), "invalid json").unwrap();
        assert!(matches!(
            refresh_remote_catalog_force(home.path(), &config).await,
            CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::Parse,
                ..
            }
        ));
        assert_eq!(load_remote_catalog_overlay(home.path()).unwrap(), before);
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }

    #[test]
    fn legacy_disk_snapshot_is_not_loaded() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("cache")).unwrap();
        std::fs::write(
            home.path().join("cache/models.dev-api.json"),
            fixture().to_string(),
        )
        .unwrap();
        assert_eq!(load_remote_catalog_overlay(home.path()).unwrap(), None);
    }
    #[tokio::test]
    async fn explicit_refresh_bypasses_startup_flag_and_replaces_removed_models() {
        let home = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), fixture().to_string()).unwrap();
        let config = CatalogConfig {
            refresh_on_startup: false,
            source: source.path().to_string_lossy().into_owned(),
            ..CatalogConfig::default()
        };
        assert_eq!(
            refresh_remote_catalog(home.path(), &config).await,
            CatalogRefreshOutcome::SkippedStartupDisabled
        );
        assert_eq!(
            refresh_remote_catalog_force(home.path(), &config).await,
            CatalogRefreshOutcome::Updated {
                providers: 1,
                models: 2
            }
        );
        let replacement = json!({"new-provider": {"npm": "@ai-sdk/openai-compatible", "models": {"replacement": {}}}});
        std::fs::write(source.path(), replacement.to_string()).unwrap();
        assert_eq!(
            refresh_remote_catalog_force(home.path(), &config).await,
            CatalogRefreshOutcome::Updated {
                providers: 1,
                models: 1
            }
        );
        let (expected, _, _) = convert_models_dev_api(&replacement).unwrap();
        assert_eq!(
            load_remote_catalog_overlay(home.path()).unwrap(),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn offline_mode_also_blocks_explicit_http_refresh() {
        let home = tempfile::tempdir().unwrap();
        let config = CatalogConfig {
            offline: true,
            ..CatalogConfig::default()
        };
        assert_eq!(
            refresh_remote_catalog_force(home.path(), &config).await,
            CatalogRefreshOutcome::SkippedOffline
        );
        assert_eq!(load_remote_catalog_overlay(home.path()).unwrap(), None);
    }
    #[test]
    fn native_sdk_defaults_apply_to_any_provider_id() {
        let api = json!({
            "unknown-openai": {"npm": "@ai-sdk/openai", "models": {"model": {}}},
            "unknown-anthropic": {"npm": "@ai-sdk/anthropic", "models": {"model": {}}},
            "unknown-google": {"npm": "@ai-sdk/google", "models": {"model": {}}},
            "unknown-compatible": {"npm": "@ai-sdk/openai-compatible", "models": {"model": {}}}
        });
        let (catalog, providers, models) = convert_models_dev_api(&api).unwrap();
        assert_eq!((providers, models), (4, 4));
        assert_eq!(
            catalog
                .providers
                .iter()
                .map(|(id, entry)| (id.as_str(), entry.base_url.as_deref(), entry.wire_api))
                .collect::<Vec<_>>(),
            vec![
                (
                    "unknown-anthropic",
                    Some("https://api.anthropic.com"),
                    Some(ProviderWireApi::AnthropicMessages)
                ),
                (
                    "unknown-compatible",
                    None,
                    Some(ProviderWireApi::OpenAIChatCompletions)
                ),
                (
                    "unknown-google",
                    Some("https://generativelanguage.googleapis.com/v1beta"),
                    Some(ProviderWireApi::GoogleGenerativeAi)
                ),
                (
                    "unknown-openai",
                    Some("https://api.openai.com/v1"),
                    Some(ProviderWireApi::OpenAIResponses)
                ),
            ]
        );
    }

    #[test]
    fn model_sdk_default_does_not_override_explicit_provider_endpoint() {
        let (catalog, _, _) = convert_models_dev_api(&json!({
            "explicit": {"npm": "@ai-sdk/openai", "api": "https://gateway.example", "models": {
                "model": {"provider": {"npm": "@ai-sdk/anthropic"}}
            }},
            "sdk-default": {"npm": "@ai-sdk/openai", "models": {
                "model": {"provider": {"npm": "@ai-sdk/anthropic"}}
            }}
        }))
        .unwrap();
        assert_eq!(
            (
                catalog.providers["explicit"].models["model"]
                    .base_url
                    .as_deref(),
                catalog.providers["sdk-default"].models["model"]
                    .base_url
                    .as_deref()
            ),
            (None, Some("https://api.anthropic.com"))
        );
    }
}
