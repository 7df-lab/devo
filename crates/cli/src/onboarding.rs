use std::io::{self, BufRead, Write};

use anyhow::Result;
use clawcr_core::{BuiltinModelCatalog, ModelCatalog, ModelConfig, ProviderKind};

use crate::config::{AppConfig, ConfiguredModel, ProviderProfile};

/// Run the first-time interactive setup wizard.
///
/// The user selects a provider family, then can add any number of built-in or
/// custom models for that provider. Each model gets its own base URL and API key.
pub fn run_onboarding() -> Result<AppConfig> {
    println!("╔══════════════════════════════════════════╗");
    println!("║      Welcome to Claw RS!                 ║");
    println!("║   Let's set up your AI provider.         ║");
    println!("╚══════════════════════════════════════════╝");
    println!();

    let provider = choose_provider()?;
    let catalog = BuiltinModelCatalog::load()?;
    let models = collect_models(provider, &catalog)?;
    let default_model = choose_default_model(&models)?;
    let profile = provider_profile(models, &default_model);

    let config = AppConfig {
        default_provider: Some(provider),
        anthropic: if provider == ProviderKind::Anthropic {
            profile.clone()
        } else {
            ProviderProfile::default()
        },
        openai: if provider == ProviderKind::Openai {
            profile.clone()
        } else {
            ProviderProfile::default()
        },
        ollama: if provider == ProviderKind::Ollama {
            profile.clone()
        } else {
            ProviderProfile::default()
        },
    };

    crate::config::save_config(&config)?;
    println!();
    println!("Config saved. You can change it later by editing ~/.clawcr/config.toml");
    println!("or by setting environment variables (ANTHROPIC_API_KEY, etc.).");
    println!();

    Ok(config)
}

fn choose_provider() -> Result<ProviderKind> {
    println!("Choose a provider family:");
    println!("  [1] Anthropic API  (Claude models)");
    println!("  [2] OpenAI-compatible API");
    println!("  [3] Ollama         (local models)");
    println!();

    match prompt_choice("Provider [1/2/3]", 1, 3)? {
        1 => Ok(ProviderKind::Anthropic),
        2 => Ok(ProviderKind::Openai),
        3 => Ok(ProviderKind::Ollama),
        _ => unreachable!(),
    }
}

fn collect_models(
    provider: ProviderKind,
    catalog: &BuiltinModelCatalog,
) -> Result<Vec<ConfiguredModel>> {
    let builtin_models: Vec<&ModelConfig> = catalog
        .list_visible()
        .into_iter()
        .filter(|model| model.provider == provider)
        .collect();

    let mut models = Vec::new();
    loop {
        println!();
        println!("Add models for {}:", provider.as_str());
        for (index, model) in builtin_models.iter().enumerate() {
            println!("  [{}] {} (built-in)", index + 1, model.display_name);
        }

        let custom_choice = builtin_models.len() as u32 + 1;
        let finish_choice = builtin_models.len() as u32 + 2;
        println!("  [{}] Custom model", custom_choice);
        println!("  [{}] Finish", finish_choice);
        println!();

        let choice = prompt_choice(
            "Model choice",
            1,
            builtin_models.len().saturating_add(2) as u32,
        )?;

        if choice >= 1 && choice <= builtin_models.len() as u32 {
            let selected = builtin_models[(choice - 1) as usize];
            models.push(prompt_model_entry(
                &selected.slug,
                provider,
                default_base_url_for_provider(provider),
            )?);
            continue;
        }

        if choice == custom_choice {
            let model_name = prompt_string("Model name")?;
            models.push(prompt_model_entry(
                &model_name,
                provider,
                default_base_url_for_provider(provider),
            )?);
            continue;
        }

        if models.is_empty() {
            println!("Please add at least one model before finishing.");
            continue;
        }

        break;
    }

    Ok(models)
}

fn prompt_model_entry(
    model_name: &str,
    provider: ProviderKind,
    default_base_url: &str,
) -> Result<ConfiguredModel> {
    println!();
    println!("Configuring {}", model_name);
    println!("Provider: {}", provider.as_str());
    let base_url = prompt_with_default("Base URL", default_base_url)?;
    let api_key = prompt_string("API key")?;

    Ok(ConfiguredModel {
        model: model_name.to_string(),
        base_url: Some(base_url),
        api_key: Some(api_key),
    })
}

fn choose_default_model(models: &[ConfiguredModel]) -> Result<String> {
    println!();
    println!("Choose the default model:");
    for (index, model) in models.iter().enumerate() {
        println!("  [{}] {}", index + 1, model.model);
    }
    println!();

    let choice = prompt_choice("Default model", 1, models.len() as u32)? as usize;
    Ok(models[choice - 1].model.clone())
}

fn provider_profile(models: Vec<ConfiguredModel>, default_model: &str) -> ProviderProfile {
    let default_entry = models
        .iter()
        .find(|entry| entry.model == default_model)
        .or_else(|| models.first());

    ProviderProfile {
        default_model: Some(default_model.to_string()),
        base_url: default_entry.and_then(|entry| entry.base_url.clone()),
        api_key: default_entry.and_then(|entry| entry.api_key.clone()),
        models,
    }
}

fn default_base_url_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "https://api.anthropic.com",
        ProviderKind::Openai => "https://api.openai.com",
        ProviderKind::Ollama => "http://localhost:11434",
    }
}

// ---------------------------------------------------------------------------
// Prompt helpers
// ---------------------------------------------------------------------------

fn prompt_choice(prompt: &str, min: u32, max: u32) -> Result<u32> {
    let stdin = io::stdin();
    loop {
        print!("{}: ", prompt);
        io::stdout().flush()?;

        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let trimmed = line.trim();

        if let Ok(n) = trimmed.parse::<u32>() {
            if n >= min && n <= max {
                return Ok(n);
            }
        }
        println!("Please enter a number between {} and {}.", min, max);
    }
}

fn prompt_string(prompt: &str) -> Result<String> {
    let stdin = io::stdin();
    loop {
        print!("{}: ", prompt);
        io::stdout().flush()?;

        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let trimmed = line.trim().to_string();

        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
        println!("This field is required.");
    }
}

fn prompt_with_default(prompt: &str, default: &str) -> Result<String> {
    print!("{} [{}]: ", prompt, default);
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();

    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}
