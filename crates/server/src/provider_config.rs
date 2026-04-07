use std::{fs, path::Path};

use anyhow::{Context, Result};
use clawcr_core::ProviderKind;
use serde::Deserialize;

use clawcr_provider::{anthropic::AnthropicProvider, openai::OpenAIProvider, ModelProvider};

/// Resolved provider bootstrap owned by the server runtime.
pub struct ResolvedServerProvider {
    /// Concrete provider used for model requests.
    pub provider: std::sync::Arc<dyn ModelProvider>,
    /// Default model slug used when a session or turn does not request one.
    pub default_model: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderProfile {
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<ModelProfile>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelProfile {
    #[serde(default)]
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AppConfigFile {
    #[serde(default)]
    default_provider: Option<ProviderKind>,
    #[serde(default)]
    anthropic: ProviderProfile,
    #[serde(default)]
    openai: ProviderProfile,
    #[serde(default)]
    ollama: ProviderProfile,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

/// Loads the server-side provider from config and environment variables.
pub fn load_server_provider(
    config_file: &Path,
    default_model: Option<&str>,
) -> Result<ResolvedServerProvider> {
    let file_config = read_provider_config(config_file).unwrap_or_default();
    let env_provider = env_non_empty("CLAWCR_PROVIDER");
    let env_model = env_non_empty("CLAWCR_MODEL");
    let env_base_url = env_non_empty("CLAWCR_BASE_URL");
    let env_api_key = env_non_empty("CLAWCR_API_KEY");

    let provider_name = env_provider
        .as_deref()
        .and_then(parse_provider_kind)
        .or_else(|| provider_for_model(&file_config, env_model.as_deref()))
        .or(file_config.default_provider)
        .or_else(|| {
            file_config
                .provider
                .as_deref()
                .and_then(parse_provider_kind)
        })
        .or_else(|| infer_default_provider(&file_config))
        .unwrap_or(ProviderKind::Openai);

    let profile = profile_for_provider(&file_config, provider_name);
    let selected_model = select_configured_model(
        profile,
        env_model.as_deref().or(file_config.model.as_deref()),
    );

    let model = env_model
        .or_else(|| selected_model.map(|model| model.model.clone()))
        .or(file_config.model.clone())
        .or_else(|| default_model.map(ToOwned::to_owned))
        .or_else(|| profile.default_model.clone())
        .or_else(|| profile.models.first().map(|model| model.model.clone()))
        .unwrap_or_else(|| default_model_for_provider(provider_name));

    let base_url = env_base_url
        .or_else(|| selected_model.and_then(|model| model.base_url.clone()))
        .or(profile.base_url.clone())
        .or(file_config.base_url.clone())
        .or_else(|| env_non_empty("ANTHROPIC_BASE_URL"))
        .or_else(|| env_non_empty("OPENAI_BASE_URL"));
    let api_key = env_api_key
        .or_else(|| selected_model.and_then(|model| model.api_key.clone()))
        .or(profile.api_key.clone())
        .or(file_config.api_key.clone())
        .or_else(|| env_non_empty("ANTHROPIC_API_KEY"))
        .or_else(|| env_non_empty("ANTHROPIC_AUTH_TOKEN"))
        .or_else(|| env_non_empty("OPENAI_API_KEY"));

    let provider = match provider_name {
        ProviderKind::Anthropic => {
            let api_key = api_key.context("anthropic provider requires an API key")?;
            if let Some(url) = base_url {
                std::sync::Arc::new(AnthropicProvider::new_with_url(api_key, url))
                    as std::sync::Arc<dyn ModelProvider>
            } else {
                std::sync::Arc::new(AnthropicProvider::new(api_key))
            }
        }
        ProviderKind::Ollama | ProviderKind::Openai => {
            let base_url = ensure_openai_v1(&base_url.unwrap_or_else(|| {
                if provider_name == ProviderKind::Ollama {
                    "http://localhost:11434".to_string()
                } else {
                    "https://api.openai.com".to_string()
                }
            }));
            let mut provider = OpenAIProvider::new(base_url);
            if let Some(api_key) = api_key {
                provider = provider.with_api_key(api_key);
            }
            std::sync::Arc::new(provider)
        }
    };

    Ok(ResolvedServerProvider {
        provider,
        default_model: model,
    })
}

fn read_provider_config(config_file: &Path) -> Result<AppConfigFile> {
    if !config_file.exists() {
        return Ok(AppConfigFile::default());
    }
    let contents = fs::read_to_string(config_file)
        .with_context(|| format!("failed to read {}", config_file.display()))?;
    let value: toml::Value = toml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", config_file.display()))?;
    let table = value.as_table().cloned().unwrap_or_default();
    if table.contains_key("default_provider")
        || table.contains_key("anthropic")
        || table.contains_key("openai")
        || table.contains_key("ollama")
        || table.contains_key("models")
        || table.contains_key("default_model")
    {
        let parsed_new: Result<AppConfigFile> =
            value.clone().try_into().map_err(anyhow::Error::from);
        if let Ok(config) = parsed_new {
            return Ok(config);
        }
        let legacy: LegacySectionAppConfig = value
            .try_into()
            .with_context(|| format!("failed to parse {}", config_file.display()))?;
        return Ok(legacy.into_app_config_file());
    }

    let legacy: LegacyFlatAppConfig = value
        .try_into()
        .with_context(|| format!("failed to parse {}", config_file.display()))?;
    Ok(legacy.into_app_config_file())
}

fn profile_for_provider(config: &AppConfigFile, provider: ProviderKind) -> &ProviderProfile {
    match provider {
        ProviderKind::Anthropic => &config.anthropic,
        ProviderKind::Openai => &config.openai,
        ProviderKind::Ollama => &config.ollama,
    }
}

fn infer_default_provider(config: &AppConfigFile) -> Option<ProviderKind> {
    if config.anthropic.default_model.is_some()
        || config.anthropic.base_url.is_some()
        || config.anthropic.api_key.is_some()
        || !config.anthropic.models.is_empty()
    {
        Some(ProviderKind::Anthropic)
    } else if config.openai.default_model.is_some()
        || config.openai.base_url.is_some()
        || config.openai.api_key.is_some()
        || !config.openai.models.is_empty()
    {
        Some(ProviderKind::Openai)
    } else if config.ollama.default_model.is_some()
        || config.ollama.base_url.is_some()
        || config.ollama.api_key.is_some()
        || !config.ollama.models.is_empty()
    {
        Some(ProviderKind::Ollama)
    } else {
        None
    }
}

fn default_model_for_provider(provider: ProviderKind) -> String {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-20250514".to_string(),
        ProviderKind::Ollama => "qwen3.5:9b".to_string(),
        ProviderKind::Openai => "gpt-4o".to_string(),
    }
}

fn select_configured_model<'a>(
    profile: &'a ProviderProfile,
    requested: Option<&str>,
) -> Option<&'a ModelProfile> {
    match requested {
        Some(model) => profile.models.iter().find(|entry| entry.model == model),
        None => profile
            .default_model
            .as_deref()
            .and_then(|default| profile.models.iter().find(|entry| entry.model == default))
            .or_else(|| profile.models.first()),
    }
}

fn provider_for_model(
    config: &AppConfigFile,
    requested_model: Option<&str>,
) -> Option<ProviderKind> {
    let requested_model = requested_model?;
    for (provider, profile) in [
        (ProviderKind::Anthropic, &config.anthropic),
        (ProviderKind::Openai, &config.openai),
        (ProviderKind::Ollama, &config.ollama),
    ] {
        if profile
            .models
            .iter()
            .any(|entry| entry.model == requested_model)
            || profile.default_model.as_deref() == Some(requested_model)
        {
            return Some(provider);
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyFlatAppConfig {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

impl LegacyFlatAppConfig {
    fn into_app_config_file(self) -> AppConfigFile {
        let provider = self
            .provider
            .as_deref()
            .and_then(parse_provider_kind)
            .unwrap_or(ProviderKind::Anthropic);
        let model = self
            .model
            .unwrap_or_else(|| default_model_for_provider(provider));
        let profile = ProviderProfile {
            default_model: Some(model.clone()),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            models: vec![ModelProfile {
                model,
                base_url: self.base_url,
                api_key: self.api_key,
            }],
        };
        match provider {
            ProviderKind::Anthropic => AppConfigFile {
                default_provider: Some(provider),
                anthropic: profile,
                openai: ProviderProfile::default(),
                ollama: ProviderProfile::default(),
                provider: None,
                model: None,
                base_url: None,
                api_key: None,
            },
            ProviderKind::Openai => AppConfigFile {
                default_provider: Some(provider),
                anthropic: ProviderProfile::default(),
                openai: profile,
                ollama: ProviderProfile::default(),
                provider: None,
                model: None,
                base_url: None,
                api_key: None,
            },
            ProviderKind::Ollama => AppConfigFile {
                default_provider: Some(provider),
                anthropic: ProviderProfile::default(),
                openai: ProviderProfile::default(),
                ollama: profile,
                provider: None,
                model: None,
                base_url: None,
                api_key: None,
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LegacySectionAppConfig {
    #[serde(default)]
    default_provider: Option<ProviderKind>,
    #[serde(default)]
    anthropic: LegacySectionProviderProfile,
    #[serde(default)]
    openai: LegacySectionProviderProfile,
    #[serde(default)]
    ollama: LegacySectionProviderProfile,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LegacySectionProviderProfile {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

impl LegacySectionAppConfig {
    fn into_app_config_file(self) -> AppConfigFile {
        AppConfigFile {
            default_provider: self.default_provider,
            anthropic: legacy_section_profile_into_provider_profile(self.anthropic),
            openai: legacy_section_profile_into_provider_profile(self.openai),
            ollama: legacy_section_profile_into_provider_profile(self.ollama),
            provider: None,
            model: None,
            base_url: None,
            api_key: None,
        }
    }
}

fn legacy_section_profile_into_provider_profile(
    legacy: LegacySectionProviderProfile,
) -> ProviderProfile {
    let model = legacy.model.clone();
    ProviderProfile {
        default_model: model.clone(),
        base_url: legacy.base_url.clone(),
        api_key: legacy.api_key.clone(),
        models: model
            .map(|model| ModelProfile {
                model,
                base_url: legacy.base_url,
                api_key: legacy.api_key,
            })
            .into_iter()
            .collect(),
    }
}

fn parse_provider_kind(value: &str) -> Option<ProviderKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" => Some(ProviderKind::Openai),
        "ollama" => Some(ProviderKind::Ollama),
        _ => None,
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn ensure_openai_v1(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}
