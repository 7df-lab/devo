use std::collections::BTreeMap;
use std::time::Instant;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ModelOverrideConfig, ProviderConfigFile};

fn legacy_apply(
    catalog: &mut ProviderConfigFile,
    overrides: &BTreeMap<String, ModelOverrideConfig>,
) {
    for (reference, patch) in overrides {
        let matches = catalog
            .providers
            .iter()
            .flat_map(|(provider_id, provider)| {
                provider.models.keys().filter_map(move |model_id| {
                    (reference == model_id || reference == &format!("{provider_id}/{model_id}"))
                        .then(|| (provider_id.clone(), model_id.clone()))
                })
            })
            .collect::<Vec<_>>();
        for (provider, model) in matches {
            catalog
                .providers
                .get_mut(&provider)
                .unwrap()
                .models
                .get_mut(&model)
                .unwrap()
                .apply_model_override(patch);
        }
    }
}

#[test]
fn indexed_overrides_preserve_bare_full_ambiguous_and_ordered_matching() {
    let mut catalog: ProviderConfigFile = serde_json::from_value(json!({
        "provider": {
            "p0": { "models": { "shared": {}, "family/model": {}, "family/model/high": {} } },
            "p1": { "models": { "shared": {}, "p0/shared": {}, "family/model": {} } }
        }
    }))
    .unwrap();
    let overrides: BTreeMap<String, ModelOverrideConfig> = serde_json::from_value(json!({
        "shared": { "context_window": 32000 },
        "p0/shared": { "max_tokens": 1024, "context_window": 64000 },
        "p1/shared": { "max_tokens": 2048 },
        "family/model": { "temperature": 0.5 },
        "p0/family/model": { "max_tokens": 4096 },
        "p0/family/model/high": { "max_tokens": 512 },
        "absent/model": { "max_tokens": 1 }
    }))
    .unwrap();
    let mut expected = catalog.clone();
    legacy_apply(&mut expected, &overrides);
    catalog.apply_model_overrides(&overrides);
    assert_eq!(catalog, expected);
    assert_eq!(
        (
            catalog.providers["p0"].models["shared"].context_window,
            catalog.providers["p1"].models["p0/shared"].context_window
        ),
        (Some(32000), Some(64000))
    );
    let unchanged = catalog.clone();
    catalog.apply_model_overrides(&BTreeMap::new());
    assert_eq!(catalog, unchanged);
}

#[test]
fn indexed_overrides_match_normalized_provider_section() {
    let mut catalog = fixture(3, 25);
    let overrides = catalog.to_provider_config_section().model_overrides;
    let mut expected = catalog.clone();
    legacy_apply(&mut expected, &overrides);
    catalog.apply_model_overrides(&overrides);
    assert_eq!(catalog, expected);
}

fn fixture(providers: usize, models: usize) -> ProviderConfigFile {
    let mut directory = serde_json::Map::new();
    for provider in 0..providers {
        let mut entries = serde_json::Map::new();
        for model in 0..models {
            entries.insert(
                format!("family/model-{model}"),
                json!({"context_window": 128000, "max_tokens": 8192}),
            );
        }
        directory.insert(format!("provider-{provider}"), json!({"models": entries}));
    }
    serde_json::from_value(json!({"provider": directory})).unwrap()
}

#[test]
#[ignore = "manual local CPU benchmark, no network or model calls"]
fn indexed_overrides_large_directory_benchmark() {
    let mut catalog = fixture(20, 300);
    let overrides = catalog.to_provider_config_section().model_overrides;
    let mut expected = catalog.clone();
    let started = Instant::now();
    legacy_apply(&mut expected, &overrides);
    let old = started.elapsed();
    let started = Instant::now();
    catalog.apply_model_overrides(&overrides);
    let optimized = started.elapsed();
    assert_eq!(catalog, expected);
    eprintln!("6000 normalized model overrides: old={old:?}, indexed={optimized:?}");
}
