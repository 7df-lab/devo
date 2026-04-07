use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clawcr_core::ProviderKind;
use clawcr_utils::{current_user_config_file, FileSystemConfigPathResolver};
use serde::{Deserialize, Serialize};

/// One model entry stored under a provider section in `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfiguredModel {
    /// The model slug or custom model name.
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// One provider-specific configuration block that can store many model entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ConfiguredModel>,
}

impl ProviderProfile {
    pub(crate) fn is_empty(&self) -> bool {
        self.default_model.is_none()
            && self.base_url.is_none()
            && self.api_key.is_none()
            && self.models.is_empty()
    }
}

/// Persisted provider configuration grouped by provider family.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "ProviderProfile::is_empty")]
    pub anthropic: ProviderProfile,
    #[serde(default, skip_serializing_if = "ProviderProfile::is_empty")]
    pub openai: ProviderProfile,
    #[serde(default, skip_serializing_if = "ProviderProfile::is_empty")]
    pub ollama: ProviderProfile,
}

/// The fully-resolved provider settings that can be forwarded to a server process.
pub struct ResolvedProviderSettings {
    /// Normalized provider name.
    pub provider: ProviderKind,
    /// Final model identifier.
    pub model: String,
    /// Optional provider base URL override.
    pub base_url: Option<String>,
    /// Optional provider API key override.
    pub api_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Config file I/O
// ---------------------------------------------------------------------------

/// `~/.clawcr/config.toml`
pub fn config_path() -> Result<PathBuf> {
    current_user_config_file().context("could not determine user config path")
}

/// The previous JSON location under the current `.clawcr` directory.
fn legacy_json_config_path() -> Result<PathBuf> {
    let resolver = FileSystemConfigPathResolver::from_env()
        .context("could not determine home directory for legacy config path")?;
    Ok(resolver.user_config_dir().join("config.json"))
}

/// The older pre-spec JSON location used by early CLI builds.
fn legacy_cli_config_path() -> Result<PathBuf> {
    let resolver = FileSystemConfigPathResolver::from_env()
        .context("could not determine home directory for legacy config path")?;
    Ok(resolver
        .user_config_dir()
        .parent()
        .expect("config dir should have a parent home directory")
        .join(".claw-code-rust")
        .join("config.json"))
}

/// Load a JSON config file from one of the legacy locations.
fn load_legacy_json_config(path: &Path) -> Result<AppConfig> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let legacy: LegacyFlatAppConfig = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(legacy.into_app_config())
}

pub fn load_config() -> Result<AppConfig> {
    let path = config_path()?;
    if path.exists() {
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value: toml::Value =
            toml::from_str(&data).with_context(|| format!("failed to parse {}", path.display()))?;
        let table = value.as_table().cloned().unwrap_or_default();
        if table.contains_key("default_provider")
            || table.contains_key("anthropic")
            || table.contains_key("openai")
            || table.contains_key("ollama")
            || table.contains_key("models")
            || table.contains_key("default_model")
        {
            let parsed_new: Result<AppConfig, toml::de::Error> = value.clone().try_into();
            if let Ok(config) = parsed_new {
                return Ok(config);
            }
            let legacy_provider_cfg: LegacySectionAppConfig = value
                .try_into()
                .with_context(|| format!("failed to parse {}", path.display()))?;
            return Ok(legacy_provider_cfg.into_app_config());
        }

        let legacy_cfg: LegacyFlatAppConfig = value
            .try_into()
            .with_context(|| format!("failed to parse {}", path.display()))?;
        return Ok(legacy_cfg.into_app_config());
    }

    let legacy_json_path = legacy_json_config_path()?;
    if legacy_json_path.exists() {
        let cfg = load_legacy_json_config(&legacy_json_path)?;
        save_config(&cfg)?;
        return Ok(cfg);
    }

    let legacy_cli_path = legacy_cli_config_path()?;
    if legacy_cli_path.exists() {
        let cfg = load_legacy_json_config(&legacy_cli_path)?;
        save_config(&cfg)?;
        return Ok(cfg);
    }

    Ok(AppConfig::default())
}

pub fn save_config(config: &AppConfig) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let toml = toml::to_string_pretty(config)?;
    std::fs::write(&path, toml).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Env-var detection
// ---------------------------------------------------------------------------

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Build a partial config from environment variables.
fn env_config() -> AppConfig {
    let anthropic_api_key =
        env_non_empty("ANTHROPIC_API_KEY").or_else(|| env_non_empty("ANTHROPIC_AUTH_TOKEN"));
    let anthropic_base_url = env_non_empty("ANTHROPIC_BASE_URL");
    let openai_api_key = env_non_empty("OPENAI_API_KEY");
    let openai_base_url = env_non_empty("OPENAI_BASE_URL");

    let default_provider = if anthropic_api_key.is_some() || anthropic_base_url.is_some() {
        Some(ProviderKind::Anthropic)
    } else if openai_api_key.is_some() || openai_base_url.is_some() {
        Some(ProviderKind::Openai)
    } else {
        None
    };

    AppConfig {
        default_provider,
        anthropic: ProviderProfile {
            base_url: anthropic_base_url,
            api_key: anthropic_api_key,
            default_model: None,
            models: Vec::new(),
        },
        openai: ProviderProfile {
            base_url: openai_base_url,
            api_key: openai_api_key,
            default_model: None,
            models: Vec::new(),
        },
        ollama: ProviderProfile::default(),
    }
}

// ---------------------------------------------------------------------------
// Provider resolution: CLI flags > env vars > config file > onboarding
// ---------------------------------------------------------------------------

/// Resolves provider settings without constructing a local provider instance.
pub fn resolve_provider_settings(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_ollama_url: &str,
    _interactive: bool,
) -> Result<ResolvedProviderSettings> {
    let env = env_config();
    let file = load_config().unwrap_or_default();

    let provider_name = cli_provider
        .and_then(|provider| parse_provider_kind(provider).ok())
        .or_else(|| cli_model.and_then(|model| provider_for_model(&file, model)))
        .or(env.default_provider)
        .or(file.default_provider)
        .or_else(|| infer_default_provider(&file));

    if let Some(provider_name) = provider_name {
        let selected_profile = profile_for_provider(&file, provider_name);
        let env_profile = profile_for_provider(&env, provider_name);
        let selected_model = select_configured_model(selected_profile, cli_model)
            .or_else(|| select_configured_model(env_profile, cli_model));
        let model = selected_model
            .map(|model| model.model.clone())
            .or_else(|| cli_model.map(str::to_string))
            .or_else(|| selected_profile.default_model.clone())
            .or_else(|| {
                selected_profile
                    .models
                    .first()
                    .map(|model| model.model.clone())
            })
            .unwrap_or_else(|| default_model_for_provider(provider_name));
        let base_url = selected_model
            .and_then(|model| model.base_url.clone())
            .or_else(|| selected_profile.base_url.clone())
            .or_else(|| env_profile.base_url.clone());
        let api_key = selected_model
            .and_then(|model| model.api_key.clone())
            .or_else(|| selected_profile.api_key.clone())
            .or_else(|| env_profile.api_key.clone());

        return Ok(ResolvedProviderSettings {
            model,
            provider: provider_name,
            base_url: normalized_base_url(cli_ollama_url, provider_name, base_url),
            api_key,
        });
    }

    anyhow::bail!(
        "No provider configured. Set ANTHROPIC_API_KEY / ANTHROPIC_AUTH_TOKEN, \
         or run `clawcr onboard` to complete setup."
    )
}

fn default_model_for_provider(provider: ProviderKind) -> String {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-20250514".to_string(),
        ProviderKind::Ollama => "qwen3.5:9b".to_string(),
        ProviderKind::Openai => "gpt-4o".to_string(),
    }
}

fn parse_provider_kind(value: &str) -> Result<ProviderKind> {
    match value.to_ascii_lowercase().as_str() {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "openai" => Ok(ProviderKind::Openai),
        "ollama" => Ok(ProviderKind::Ollama),
        other => anyhow::bail!("unknown provider `{other}`"),
    }
}

fn infer_default_provider(config: &AppConfig) -> Option<ProviderKind> {
    if !config.anthropic.is_empty() {
        Some(ProviderKind::Anthropic)
    } else if !config.openai.is_empty() {
        Some(ProviderKind::Openai)
    } else if !config.ollama.is_empty() {
        Some(ProviderKind::Ollama)
    } else {
        None
    }
}

pub fn profile_for_provider(config: &AppConfig, provider: ProviderKind) -> &ProviderProfile {
    match provider {
        ProviderKind::Anthropic => &config.anthropic,
        ProviderKind::Openai => &config.openai,
        ProviderKind::Ollama => &config.ollama,
    }
}

fn select_configured_model<'a>(
    profile: &'a ProviderProfile,
    requested_model: Option<&str>,
) -> Option<&'a ConfiguredModel> {
    match requested_model {
        Some(model) => profile.models.iter().find(|entry| entry.model == model),
        None => profile
            .default_model
            .as_deref()
            .and_then(|default_model| {
                profile
                    .models
                    .iter()
                    .find(|entry| entry.model == default_model)
            })
            .or_else(|| profile.models.first()),
    }
}

fn provider_for_model(config: &AppConfig, requested_model: &str) -> Option<ProviderKind> {
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
    fn into_app_config(self) -> AppConfig {
        let provider = self
            .provider
            .and_then(|value| parse_provider_kind(&value).ok());
        let model = self.model.unwrap_or_else(|| {
            provider
                .map(default_model_for_provider)
                .unwrap_or_else(|| default_model_for_provider(ProviderKind::Anthropic))
        });
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let profile = ProviderProfile {
            default_model: Some(model.clone()),
            base_url: base_url.clone(),
            api_key: api_key.clone(),
            models: vec![ConfiguredModel {
                model,
                base_url,
                api_key,
            }],
        };
        let default_provider = provider.or_else(|| {
            if profile.api_key.is_some()
                || profile.base_url.is_some()
                || profile.default_model.is_some()
                || !profile.models.is_empty()
            {
                Some(ProviderKind::Anthropic)
            } else {
                None
            }
        });

        match default_provider {
            Some(ProviderKind::Anthropic) => AppConfig {
                default_provider,
                anthropic: profile,
                openai: ProviderProfile::default(),
                ollama: ProviderProfile::default(),
            },
            Some(ProviderKind::Openai) => AppConfig {
                default_provider,
                anthropic: ProviderProfile::default(),
                openai: profile,
                ollama: ProviderProfile::default(),
            },
            Some(ProviderKind::Ollama) => AppConfig {
                default_provider,
                anthropic: ProviderProfile::default(),
                openai: ProviderProfile::default(),
                ollama: profile,
            },
            None => AppConfig::default(),
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
    fn into_app_config(self) -> AppConfig {
        AppConfig {
            default_provider: self.default_provider,
            anthropic: legacy_section_profile_into_provider_profile(self.anthropic),
            openai: legacy_section_profile_into_provider_profile(self.openai),
            ollama: legacy_section_profile_into_provider_profile(self.ollama),
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
            .map(|model| ConfiguredModel {
                model,
                base_url: legacy.base_url,
                api_key: legacy.api_key,
            })
            .into_iter()
            .collect(),
    }
}

fn normalized_base_url(
    cli_ollama_url: &str,
    provider_name: ProviderKind,
    base_url: Option<String>,
) -> Option<String> {
    match provider_name {
        ProviderKind::Ollama => Some(ensure_openai_v1(
            base_url.as_deref().unwrap_or(cli_ollama_url),
        )),
        ProviderKind::Openai => Some(ensure_openai_v1(
            base_url.as_deref().unwrap_or("https://api.openai.com"),
        )),
        _ => base_url,
    }
}

/// async-openai appends `/chat/completions` to the base URL, so Ollama/OpenAI
/// endpoints need a `/v1` suffix. Append it if missing.
fn ensure_openai_v1(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{}/v1", trimmed)
    }
}

// ---------------------------------------------------------------------------
// Ollama availability check + auto-start
// ---------------------------------------------------------------------------

/// Parse host and port from an Ollama URL (e.g. "http://localhost:11434").
fn parse_ollama_addr(url: &str) -> (String, u16) {
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let without_path = without_scheme.split('/').next().unwrap_or(without_scheme);
    if let Some((host, port_str)) = without_path.rsplit_once(':') {
        let port = port_str.parse().unwrap_or(11434);
        (host.to_string(), port)
    } else {
        (without_path.to_string(), 11434)
    }
}

/// Check if Ollama is listening on the given URL.
fn is_ollama_reachable(url: &str) -> bool {
    let (host, port) = parse_ollama_addr(url);
    let addr = format!("{}:{}", host, port);
    std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        std::time::Duration::from_secs(2),
    )
    .is_ok()
}

/// Ensure Ollama is running. If not, offer to start it (interactive mode)
/// or return an error (non-interactive).
pub fn ensure_ollama(url: &str, interactive: bool) -> Result<()> {
    if is_ollama_reachable(url) {
        return Ok(());
    }

    if !interactive {
        anyhow::bail!(
            "Ollama is not running at {}. Start it with `ollama serve` and try again.",
            url
        );
    }

    eprint!(
        "Ollama is not running at {}. Start it automatically? [Y/n] ",
        url
    );
    std::io::Write::flush(&mut std::io::stderr())?;

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    if !answer.is_empty() && answer != "y" && answer != "yes" {
        anyhow::bail!("Ollama is required. Start it with `ollama serve` and try again.");
    }

    eprintln!("Starting Ollama...");
    let child = std::process::Command::new("ollama")
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match child {
        Ok(_) => {
            // Wait for Ollama to become available (up to 15 seconds)
            for i in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if is_ollama_reachable(url) {
                    eprintln!("Ollama is ready. (took ~{}s)", (i + 1) / 2);
                    return Ok(());
                }
            }
            anyhow::bail!("Ollama was started but did not become reachable within 15 seconds.")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "Could not find `ollama` in PATH. \
                 Install it from https://ollama.com and try again."
            )
        }
        Err(e) => {
            anyhow::bail!("Failed to start Ollama: {}", e)
        }
    }
}
