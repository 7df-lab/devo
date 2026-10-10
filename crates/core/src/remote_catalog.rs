//! Runtime refresh of the built-in provider directory from models.dev.
//!
//! Devo always embeds `crates/core/providers.json`. On startup (unless
//! `[catalog].offline = true`), it may download `https://models.dev/api.json`
//! (or read a local dump), convert overlapping providers into a sparse overlay,
//! and cache the result under `$DEVO_HOME/cache/`.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::builtin_provider_config;
use crate::{
    CatalogConfig, InputModality, ProviderConfigEntry, ProviderConfigFile, ProviderModelConfig,
    ReasoningCapability, ReasoningEffort, levels_with_leading_off,
};

const CACHE_DIR_NAME: &str = "cache";
/// Raw models.dev dump (source of truth on disk).
const RAW_CACHE_FILE_NAME: &str = "models.dev-api.json";
/// Stale converted overlay from older Devo builds; deleted on refresh, never read.
const STALE_OVERLAY_CACHE_FILE_NAME: &str = "models.dev-catalog.json";
const META_CACHE_FILE_NAME: &str = "models.dev-meta.json";
const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/api.json";

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
    CacheWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogCacheMeta {
    fetched_at: DateTime<Utc>,
    source: String,
    providers: usize,
    models: usize,
}

/// Cache directory under the user Devo home (`$DEVO_HOME/cache`).
pub fn catalog_cache_dir(home_dir: &Path) -> PathBuf {
    home_dir.join(CACHE_DIR_NAME)
}

/// Path to the raw models.dev api.json cache.
pub fn remote_catalog_api_path(home_dir: &Path) -> PathBuf {
    catalog_cache_dir(home_dir).join(RAW_CACHE_FILE_NAME)
}

/// Loads a models.dev overlay for catalog merge.
///
/// Converts `$DEVO_HOME/cache/models.dev-api.json` in memory. Returns `Ok(None)`
/// when that dump is absent — there is no converted-file fallback.
pub fn load_remote_catalog_overlay(home_dir: &Path) -> anyhow::Result<Option<ProviderConfigFile>> {
    let api_path = remote_catalog_api_path(home_dir);
    if !api_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&api_path)?;
    let api: Value = serde_json::from_str(&raw)?;
    let builtin =
        builtin_provider_config().map_err(|error| anyhow::anyhow!("builtin catalog: {error}"))?;
    let (overlay, _, _) = convert_models_dev_api(&api, &builtin)
        .map_err(|error| anyhow::anyhow!("convert models.dev api.json: {error}"))?;
    Ok(Some(overlay))
}

/// Refresh the cached models.dev overlay according to `[catalog]` settings.
pub async fn refresh_remote_catalog(
    home_dir: &Path,
    config: &CatalogConfig,
) -> CatalogRefreshOutcome {
    if !config.refresh_on_startup {
        return CatalogRefreshOutcome::SkippedStartupDisabled;
    }

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

    if is_url && cache_is_fresh(home_dir, source, config.refresh_interval_hours) {
        return CatalogRefreshOutcome::CacheFresh;
    }

    let raw = if is_url {
        match download_models_dev(source).await {
            Ok(raw) => raw,
            Err(outcome) => return outcome,
        }
    } else {
        match fs::read_to_string(source) {
            Ok(raw) => raw,
            Err(error) => {
                return CatalogRefreshOutcome::Failed {
                    stage: CatalogRefreshStage::ReadSource,
                    message: format!("failed to read local catalog source `{source}`: {error}"),
                };
            }
        }
    };

    let api: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            return CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::Parse,
                message: error.to_string(),
            };
        }
    };

    let builtin = match builtin_provider_config() {
        Ok(builtin) => builtin,
        Err(error) => {
            return CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::Convert,
                message: error.to_string(),
            };
        }
    };

    let (_overlay, providers, models) = match convert_models_dev_api(&api, &builtin) {
        Ok(result) => result,
        Err(error) => {
            return CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::Convert,
                message: error,
            };
        }
    };

    if let Err(outcome) = write_catalog_cache(home_dir, source, &raw, providers, models) {
        return outcome;
    }

    CatalogRefreshOutcome::Updated { providers, models }
}

fn cache_is_fresh(home_dir: &Path, source: &str, interval_hours: u64) -> bool {
    let meta_path = catalog_cache_dir(home_dir).join(META_CACHE_FILE_NAME);
    let Ok(data) = fs::read_to_string(meta_path) else {
        return false;
    };
    let Ok(meta) = serde_json::from_str::<CatalogCacheMeta>(&data) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(meta.fetched_at);
    interval_hours > 0
        && meta.source == source
        && remote_catalog_api_path(home_dir).exists()
        && age.num_seconds() >= 0
        && age.num_hours() < interval_hours as i64
}

async fn download_models_dev(url: &str) -> Result<String, CatalogRefreshOutcome> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("devo/{}", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Err(CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::HttpRequest,
                message: error.to_string(),
            });
        }
    };
    let response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) => {
            return Err(CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::HttpRequest,
                message: error.to_string(),
            });
        }
    };
    if !response.status().is_success() {
        return Err(CatalogRefreshOutcome::Failed {
            stage: CatalogRefreshStage::HttpStatus,
            message: format!("models.dev returned HTTP {}", response.status()),
        });
    }
    match response.text().await {
        Ok(text) => Ok(text),
        Err(error) => Err(CatalogRefreshOutcome::Failed {
            stage: CatalogRefreshStage::HttpRequest,
            message: error.to_string(),
        }),
    }
}

fn write_catalog_cache(
    home_dir: &Path,
    source: &str,
    raw: &str,
    providers: usize,
    models: usize,
) -> Result<(), CatalogRefreshOutcome> {
    let cache_dir = catalog_cache_dir(home_dir);
    if let Err(error) = fs::create_dir_all(&cache_dir) {
        return Err(CatalogRefreshOutcome::Failed {
            stage: CatalogRefreshStage::CacheWrite,
            message: error.to_string(),
        });
    }

    let raw_path = cache_dir.join(RAW_CACHE_FILE_NAME);
    let meta_path = cache_dir.join(META_CACHE_FILE_NAME);

    if let Err(error) = write_atomic_text(&raw_path, raw) {
        return Err(CatalogRefreshOutcome::Failed {
            stage: CatalogRefreshStage::CacheWrite,
            message: error,
        });
    }
    let meta = CatalogCacheMeta {
        fetched_at: Utc::now(),
        source: source.to_string(),
        providers,
        models,
    };
    let meta_json = match serde_json::to_string_pretty(&meta) {
        Ok(mut json) => {
            json.push('\n');
            json
        }
        Err(error) => {
            return Err(CatalogRefreshOutcome::Failed {
                stage: CatalogRefreshStage::CacheWrite,
                message: error.to_string(),
            });
        }
    };
    if let Err(error) = write_atomic_text(&meta_path, &meta_json) {
        return Err(CatalogRefreshOutcome::Failed {
            stage: CatalogRefreshStage::CacheWrite,
            message: error,
        });
    }
    // Drop any leftover converted overlay from older builds.
    let stale = cache_dir.join(STALE_OVERLAY_CACHE_FILE_NAME);
    let _ = fs::remove_file(stale);
    Ok(())
}

fn write_atomic_text(path: &Path, data: &str) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog.tmp");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nanos));
    fs::write(&temp_path, data).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        error.to_string()
    })
}

/// Converts models.dev `api.json` into a Devo overlay for providers that already
/// exist in the embedded builtin directory. Wire API / base URL stay on the
/// builtin entry; only model metadata is refreshed/added.
pub fn convert_models_dev_api(
    api: &Value,
    builtin: &ProviderConfigFile,
) -> Result<(ProviderConfigFile, usize, usize), String> {
    let root = api
        .as_object()
        .ok_or_else(|| "models.dev api.json root must be an object".to_string())?;

    let mut overlay = ProviderConfigFile::default();
    let mut provider_count = 0usize;
    let mut model_count = 0usize;

    for (provider_id, builtin_entry) in &builtin.providers {
        let Some(remote_provider) = root.get(provider_id) else {
            continue;
        };
        let Some(remote_models) = remote_provider.get("models").and_then(Value::as_object) else {
            continue;
        };

        let mut entry = ProviderConfigEntry::default();
        if let Some(name) = remote_provider.get("name").and_then(Value::as_str) {
            entry.name = Some(name.to_string());
        }
        if let Some(env) = remote_provider.get("env").and_then(Value::as_array) {
            entry.env = env
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }

        for (model_id, remote_model) in remote_models {
            if remote_model.get("tool_call") == Some(&Value::Bool(false)) {
                continue;
            }
            if remote_model
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.eq_ignore_ascii_case("deprecated"))
            {
                continue;
            }
            let converted = model_from_models_dev(remote_model, builtin_entry.models.get(model_id));
            entry.models.insert(model_id.clone(), converted);
            model_count += 1;
        }

        if entry.models.is_empty() {
            continue;
        }
        overlay.providers.insert(provider_id.clone(), entry);
        provider_count += 1;
    }

    Ok((overlay, provider_count, model_count))
}

fn model_from_models_dev(
    remote: &Value,
    existing: Option<&ProviderModelConfig>,
) -> ProviderModelConfig {
    let mut model = existing.cloned().unwrap_or_default();
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

/// Splits novel (non-builtin) providers out of user `providers.json` into
/// `custom-providers.json`.
pub fn migrate_custom_providers_file(
    user_config_dir: &Path,
    builtin: &ProviderConfigFile,
) -> anyhow::Result<bool> {
    use crate::{
        CUSTOM_PROVIDER_CONFIG_FILE_NAME, PROVIDER_CONFIG_FILE_NAME, read_provider_catalog_config,
        write_provider_catalog_config,
    };

    let connection_path = user_config_dir.join(PROVIDER_CONFIG_FILE_NAME);
    let custom_path = user_config_dir.join(CUSTOM_PROVIDER_CONFIG_FILE_NAME);
    if !connection_path.exists() {
        return Ok(false);
    }

    let mut connection = read_provider_catalog_config(&connection_path)?;
    let mut custom = read_provider_catalog_config(&custom_path)?;
    let mut changed = false;

    let custom_ids = connection
        .providers
        .keys()
        .filter(|id| !builtin.providers.contains_key(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for id in custom_ids {
        let Some(moved) = connection.providers.remove(&id) else {
            continue;
        };
        match custom.providers.get_mut(&id) {
            Some(existing) => {
                // The id already has a Connection here. Merge, never replace:
                // the custom entry is the authoritative full Connection, and a
                // sparse fragment split out of `providers.json` (e.g. a
                // credential-bind stub) only refreshes the credential/enabled
                // and fills fields the custom entry lacks.
                if moved.credential.is_some() {
                    existing.credential = moved.credential;
                }
                if moved.enabled.is_some() {
                    existing.enabled = moved.enabled;
                }
                if existing.name.is_none() {
                    existing.name = moved.name;
                }
                if existing.description.is_none() {
                    existing.description = moved.description;
                }
                if existing.base_url.is_none() {
                    existing.base_url = moved.base_url;
                }
                if existing.wire_api.is_none() {
                    existing.wire_api = moved.wire_api;
                }
                if existing.headers.is_none() {
                    existing.headers = moved.headers;
                }
                if existing.options.is_none() {
                    existing.options = moved.options;
                }
                if existing.request.is_none() {
                    existing.request = moved.request;
                }
                if existing.compat.is_none() {
                    existing.compat = moved.compat;
                }
                for (model_id, model) in moved.models {
                    existing.models.entry(model_id).or_insert(model);
                }
                for (model_id, model_override) in moved.model_overrides {
                    existing
                        .model_overrides
                        .entry(model_id)
                        .or_insert(model_override);
                }
            }
            None => {
                custom.providers.insert(id, moved);
            }
        }
        changed = true;
    }

    if changed {
        write_provider_catalog_config(&connection_path, &connection)?;
        write_provider_catalog_config(&custom_path, &custom)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::CUSTOM_PROVIDER_CONFIG_FILE_NAME;
    use crate::ModelCatalog;
    use crate::PROVIDER_CONFIG_FILE_NAME;
    use crate::ProviderWireApi;
    use crate::read_provider_catalog_config;
    use crate::write_provider_catalog_config;

    #[test]
    fn fresh_cache_requires_the_same_source_and_a_complete_dump() {
        let dir = tempdir().expect("tempdir");
        let source = "https://catalog.example/api.json";
        write_catalog_cache(dir.path(), source, "{}", 0, 0).expect("cache");
        assert_eq!(
            [
                cache_is_fresh(dir.path(), source, 24),
                cache_is_fresh(dir.path(), source, 0),
                cache_is_fresh(dir.path(), "https://other.example/api.json", 24),
            ],
            [true, false, false],
        );
        fs::remove_file(remote_catalog_api_path(dir.path())).expect("remove fixture dump");
        assert!(!cache_is_fresh(dir.path(), source, 24));
    }

    #[test]
    fn remote_model_ownership_survives_user_overrides() {
        let mut model = model_from_models_dev(&json!({"name": "Remote"}), /*existing*/ None);
        model.apply_overlay(ProviderModelConfig {
            origin: Some(devo_protocol::ProviderModelOrigin::User),
            enabled: Some(false),
            ..ProviderModelConfig::default()
        });
        assert_eq!(
            model,
            ProviderModelConfig {
                origin: Some(devo_protocol::ProviderModelOrigin::Remote),
                name: Some("Remote".to_string()),
                enabled: Some(false),
                ..ProviderModelConfig::default()
            }
        );
    }

    #[test]
    fn convert_updates_overlapping_builtin_models_only() {
        let builtin = ProviderConfigFile {
            providers: BTreeMap::from([(
                "anthropic".to_string(),
                ProviderConfigEntry {
                    name: Some("Anthropic".to_string()),
                    wire_api: Some(ProviderWireApi::AnthropicMessages),
                    base_url: Some("https://api.anthropic.com".to_string()),
                    models: BTreeMap::from([(
                        "claude-sonnet-4-6".to_string(),
                        ProviderModelConfig {
                            name: Some("Old".to_string()),
                            context_window: Some(1000),
                            ..ProviderModelConfig::default()
                        },
                    )]),
                    ..ProviderConfigEntry::default()
                },
            )]),
            ..ProviderConfigFile::default()
        };
        let api = json!({
            "anthropic": {
                "name": "Anthropic",
                "env": ["ANTHROPIC_API_KEY"],
                "models": {
                    "claude-sonnet-4-6": {
                        "id": "claude-sonnet-4-6",
                        "name": "Claude Sonnet 4.6",
                        "reasoning": true,
                        "tool_call": true,
                        "limit": { "context": 200000, "output": 64000 },
                        "modalities": { "input": ["text", "image"] },
                        "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 }
                    },
                    "skip-me": {
                        "id": "skip-me",
                        "name": "Skipped",
                        "tool_call": false
                    }
                }
            },
            "unknown-provider": {
                "name": "Unknown",
                "models": {
                    "x": { "id": "x", "name": "X", "tool_call": true }
                }
            }
        });

        let (overlay, providers, models) = convert_models_dev_api(&api, &builtin).expect("convert");
        assert_eq!(providers, 1);
        assert_eq!(models, 1);
        let anthropic = overlay.providers.get("anthropic").expect("anthropic");
        assert!(anthropic.base_url.is_none());
        assert!(anthropic.wire_api.is_none());
        let model = anthropic.models.get("claude-sonnet-4-6").expect("model");
        assert_eq!(model.name.as_deref(), Some("Claude Sonnet 4.6"));
        assert_eq!(model.context_window, Some(200000));
        assert_eq!(model.max_tokens, Some(64000));
        assert_eq!(model.reasoning, Some(true));
        assert!(!overlay.providers.contains_key("unknown-provider"));
    }

    #[tokio::test]
    async fn offline_skips_network_url_source() {
        let dir = tempdir().expect("tempdir");
        let outcome = refresh_remote_catalog(
            dir.path(),
            &CatalogConfig {
                offline: true,
                source: DEFAULT_MODELS_DEV_URL.to_string(),
                refresh_on_startup: true,
                refresh_interval_hours: 24,
            },
        )
        .await;
        assert_eq!(outcome, CatalogRefreshOutcome::SkippedOffline);
    }

    #[tokio::test]
    async fn local_source_works_while_offline() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("api.json");
        let api = json!({
            "anthropic": {
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4-6": {
                        "id": "claude-sonnet-4-6",
                        "name": "Claude Sonnet 4.6",
                        "tool_call": true,
                        "limit": { "context": 111111, "output": 2222 }
                    }
                }
            }
        });
        fs::write(&source, serde_json::to_string(&api).unwrap()).unwrap();

        // Seed a minimal builtin overlap by writing converted cache via refresh.
        // builtin_provider_config() is the real embedded catalog; anthropic exists there.
        let outcome = refresh_remote_catalog(
            dir.path(),
            &CatalogConfig {
                offline: true,
                source: source.to_string_lossy().to_string(),
                refresh_on_startup: true,
                refresh_interval_hours: 24,
            },
        )
        .await;
        match outcome {
            CatalogRefreshOutcome::Updated { providers, models } => {
                assert!(providers >= 1);
                assert!(models >= 1);
            }
            other => panic!("expected Updated, got {other:?}"),
        }
        let overlay = load_remote_catalog_overlay(dir.path())
            .expect("load")
            .expect("overlay present");
        assert!(overlay.providers.contains_key("anthropic"));
    }

    #[tokio::test]
    async fn real_models_dev_fixture_converts_when_present() {
        let fixture = std::env::temp_dir().join("models.dev-api.json");
        if !fixture.exists() {
            return;
        }
        let dir = tempdir().expect("tempdir");
        let outcome = refresh_remote_catalog(
            dir.path(),
            &CatalogConfig {
                offline: true,
                source: fixture.to_string_lossy().to_string(),
                refresh_on_startup: true,
                refresh_interval_hours: 1,
            },
        )
        .await;
        let CatalogRefreshOutcome::Updated { providers, models } = outcome else {
            panic!("expected Updated from real fixture, got {outcome:?}");
        };
        assert!(providers >= 20, "providers={providers}");
        assert!(models >= 100, "models={models}");
        let overlay = load_remote_catalog_overlay(dir.path())
            .expect("load")
            .expect("overlay");
        let anthropic = overlay.providers.get("anthropic").expect("anthropic");
        assert!(anthropic.models.contains_key("claude-sonnet-4-6"));
        let catalog = crate::PresetModelCatalog::load_from_provider_config_with_home(
            &ProviderConfigFile::default(),
            &Default::default(),
            Some(dir.path()),
        )
        .expect("catalog");
        let model = catalog
            .resolve_for_turn(Some("anthropic/claude-sonnet-4-6"))
            .expect("resolve");
        assert_eq!(model.slug, "anthropic/claude-sonnet-4-6");
        assert!(model.context_window >= 100_000);
    }
    #[test]
    fn migrate_merges_stub_without_clobbering_full_custom_entry() {
        let dir = tempdir().expect("tempdir");
        let full = ProviderConfigEntry {
            name: Some("CC Proxy".to_string()),
            base_url: Some("https://example/api".to_string()),
            credential: None,
            wire_api: Some(ProviderWireApi::AnthropicMessages),
            enabled: Some(true),
            models: BTreeMap::from([(
                "glm-5.3".to_string(),
                ProviderModelConfig {
                    wire_api: Some(ProviderWireApi::AnthropicMessages),
                    ..ProviderModelConfig::default()
                },
            )]),
            ..ProviderConfigEntry::default()
        };
        // A credential-bind stub that landed in providers.json for a provider
        // whose full Connection lives in custom-providers.json.
        let stub = ProviderConfigEntry {
            name: Some("ccproxy".to_string()),
            credential: Some("ccproxy".to_string()),
            enabled: Some(true),
            ..ProviderConfigEntry::default()
        };
        write_provider_catalog_config(
            &dir.path().join(PROVIDER_CONFIG_FILE_NAME),
            &ProviderConfigFile {
                providers: BTreeMap::from([("ccproxy".to_string(), stub)]),
                ..ProviderConfigFile::default()
            },
        )
        .expect("write providers.json");
        write_provider_catalog_config(
            &dir.path().join(CUSTOM_PROVIDER_CONFIG_FILE_NAME),
            &ProviderConfigFile {
                providers: BTreeMap::from([("ccproxy".to_string(), full.clone())]),
                ..ProviderConfigFile::default()
            },
        )
        .expect("write custom-providers.json");

        let changed = migrate_custom_providers_file(dir.path(), &ProviderConfigFile::default())
            .expect("migrate");
        assert!(
            changed,
            "the stub must still be split out of providers.json"
        );

        let connection = read_provider_catalog_config(&dir.path().join(PROVIDER_CONFIG_FILE_NAME))
            .expect("read providers.json");
        let custom =
            read_provider_catalog_config(&dir.path().join(CUSTOM_PROVIDER_CONFIG_FILE_NAME))
                .expect("read custom-providers.json");
        assert!(
            connection.providers.is_empty(),
            "stub must leave providers.json: {connection:?}"
        );

        let expected = ProviderConfigEntry {
            credential: Some("ccproxy".to_string()),
            ..full
        };
        assert_eq!(
            custom.providers.get("ccproxy"),
            Some(&expected),
            "the full custom entry must survive with the fresh credential"
        );
    }
}
