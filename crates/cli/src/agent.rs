use anyhow::{Context, Result};
use clawcr_core::{
    ModelCatalog, PresetModelCatalog, ProviderConfigFile, ResolvedProviderSettings, load_config,
    resolve_provider_settings,
};
use clawcr_protocol::ProviderFamily;
use clawcr_tui::{InteractiveTuiConfig, SavedModelEntry, TerminalMode, run_interactive_tui};

/// Runs the interactive coding-agent entrypoint.
pub async fn run_agent(force_onboarding: bool, no_alt_screen: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let model_catalog = PresetModelCatalog::load()?;
    let stored_config = load_config().unwrap_or_default();
    let (onboarding_mode, resolved) =
        resolve_initial_provider_settings(force_onboarding, &stored_config, &model_catalog)?;
    let saved_models = [
        (ProviderFamily::Anthropic, &stored_config.anthropic.models),
        (ProviderFamily::OpenAI, &stored_config.openai.models),
    ]
    .into_iter()
    .flat_map(|(provider, models)| {
        models.iter().map(move |model| SavedModelEntry {
            model: model.model.clone(),
            provider,
            base_url: model.base_url.clone(),
            api_key: model.api_key.clone(),
        })
    })
    .collect();

    let server_env = server_env_overrides(&resolved);
    let clawcr_core::ResolvedProviderSettings {
        provider,
        model,
        base_url: _,
        api_key: _,
    } = resolved;

    run_interactive_tui(InteractiveTuiConfig {
        model,
        provider,
        cwd,
        server_env,
        model_catalog,
        saved_models,
        show_model_onboarding: onboarding_mode,
        terminal_mode: if no_alt_screen {
            TerminalMode::Never
        } else {
            TerminalMode::Auto
        },
    })
    .await
    .map(|_| ())
}

fn resolve_initial_provider_settings(
    force_onboarding: bool,
    stored_config: &ProviderConfigFile,
    model_catalog: &PresetModelCatalog,
) -> Result<(bool, ResolvedProviderSettings)> {
    let onboarding_mode = force_onboarding
        || (stored_config.anthropic.is_empty()
            && stored_config.openai.is_empty()
            && stored_config.ollama.is_empty());
    let resolved = if onboarding_mode {
        let fallback_model = model_catalog
            .resolve_for_turn(None)
            .context("builtin model catalog does not contain a visible onboarding model")?;
        ResolvedProviderSettings {
            provider: fallback_model.provider_family,
            model: fallback_model.slug.clone(),
            base_url: None,
            api_key: None,
        }
    } else {
        resolve_provider_settings()
            .with_context(|| "failed to resolve provider settings outside onboarding mode")?
    };
    Ok((onboarding_mode, resolved))
}

fn server_env_overrides(resolved: &clawcr_core::ResolvedProviderSettings) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "CLAWCR_PROVIDER".to_string(),
            resolved.provider.as_str().to_string(),
        ),
        ("CLAWCR_MODEL".to_string(), resolved.model.clone()),
    ];
    if let Some(base_url) = &resolved.base_url {
        env.push(("CLAWCR_BASE_URL".to_string(), base_url.clone()));
    }
    if let Some(api_key) = &resolved.api_key {
        env.push(("CLAWCR_API_KEY".to_string(), api_key.clone()));
    }
    env
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::resolve_initial_provider_settings;
    use clawcr_core::{Model, PresetModelCatalog, ProviderConfigFile};
    use clawcr_protocol::ProviderFamily;

    fn test_catalog() -> PresetModelCatalog {
        PresetModelCatalog::new(vec![Model {
            slug: "test-onboard-model".to_string(),
            provider_family: ProviderFamily::OpenAI,
            ..Model::default()
        }])
    }

    #[test]
    fn resolve_initial_provider_settings_uses_catalog_fallback_during_onboarding() {
        let (onboarding_mode, resolved) = resolve_initial_provider_settings(
            false,
            &ProviderConfigFile::default(),
            &test_catalog(),
        )
        .expect("resolve initial provider settings");

        assert!(onboarding_mode);
        assert_eq!(resolved.provider, ProviderFamily::OpenAI);
        assert_eq!(resolved.model, "test-onboard-model");
        assert_eq!(resolved.base_url, None);
        assert_eq!(resolved.api_key, None);
    }
}
