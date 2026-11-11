//! Linear-size projection of provider model metadata for Native catalogs.
use std::collections::BTreeMap;

use devo_core::{ModelCatalog, ModelCatalogEntry};
use devo_protocol::native::rpc_admin::ModelInfo;

pub(super) fn model_infos_from_catalog(catalog: &dyn ModelCatalog) -> Vec<ModelInfo> {
    // list_provider_models returns an owned map. Load each provider once, not once
    // per model: cloning every sibling map makes large model directories quadratic.
    let mut provider_models = BTreeMap::new();
    catalog
        .list_visible()
        .into_iter()
        .map(|model| model_info_from_catalog_model(model, catalog, &mut provider_models))
        .collect()
}

fn model_info_from_catalog_model(
    model: &devo_protocol::Model,
    catalog: &dyn ModelCatalog,
    provider_models: &mut BTreeMap<String, BTreeMap<String, devo_core::ProviderModelInfo>>,
) -> devo_protocol::native::rpc_admin::ModelInfo {
    // Prefer the catalog Model's authored thinkingLevelMap (pi-ai
    // getSupportedThinkingLevels), not capability-only derivation from
    // ModelCatalogEntry which drops the map.
    let (reasoning, thinking_level_map, available_thinking_levels) =
        devo_protocol::resolve_thinking_fields_for_model_info(
            &model.reasoning_capability,
            model.reasoning,
            model.thinking_level_map.as_ref(),
        );
    let mut info =
        devo_protocol::native::rpc_admin::ModelInfo::from(ModelCatalogEntry::from(model));
    info.reasoning = reasoning;
    info.thinking_level_map = if thinking_level_map.is_empty() {
        None
    } else {
        Some(thinking_level_map)
    };
    info.available_thinking_levels = available_thinking_levels;
    let Some((provider_id, model_id)) = model.slug.split_once('/') else {
        return info;
    };
    let models = provider_models
        .entry(provider_id.to_string())
        .or_insert_with(|| catalog.list_provider_models(provider_id));
    let Some(metadata) = models.get(model_id).cloned() else {
        return info;
    };
    info.with_provider_metadata(provider_id.to_string(), model_id.to_string(), metadata)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use devo_core::{Model, ModelError, ProviderModelInfo};
    use pretty_assertions::assert_eq;

    use super::*;

    struct CountingCatalog {
        models: Vec<Model>,
        metadata: BTreeMap<String, BTreeMap<String, ProviderModelInfo>>,
        reads: Mutex<BTreeMap<String, usize>>,
    }

    impl CountingCatalog {
        fn fixture(providers: usize, models_per_provider: usize) -> Self {
            let mut models = Vec::new();
            let mut metadata = BTreeMap::new();
            for provider in 0..providers {
                let provider_id = format!("provider-{provider}");
                let mut entries = BTreeMap::new();
                for model in 0..models_per_provider {
                    let model_id = format!("family/model-{model}");
                    models.push(Model {
                        slug: format!("{provider_id}/{model_id}"),
                        display_name: format!("Model {model}"),
                        reasoning: Some(true),
                        thinking_level_map: Some(BTreeMap::from([
                            ("low".to_string(), Some("minimal".to_string())),
                            ("high".to_string(), Some("deep".to_string())),
                        ])),
                        ..Model::default()
                    });
                    entries.insert(
                        model_id,
                        ProviderModelInfo {
                            context_window: Some(128_000),
                            max_tokens: Some(8192),
                            base_instructions: Some("test instructions".repeat(8)),
                            ..ProviderModelInfo::default()
                        },
                    );
                }
                metadata.insert(provider_id, entries);
            }
            Self {
                models,
                metadata,
                reads: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl ModelCatalog for CountingCatalog {
        fn list_visible(&self) -> Vec<&Model> {
            self.models.iter().collect()
        }
        fn list_provider_models(&self, provider_id: &str) -> BTreeMap<String, ProviderModelInfo> {
            *self
                .reads
                .lock()
                .unwrap()
                .entry(provider_id.to_string())
                .or_default() += 1;
            self.metadata.get(provider_id).cloned().unwrap_or_default()
        }
        fn get(&self, slug: &str) -> Option<&Model> {
            self.models.iter().find(|model| model.slug == slug)
        }
        fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError> {
            match requested {
                Some(slug) => self.get(slug).ok_or_else(|| ModelError::ModelNotFound {
                    slug: slug.to_string(),
                }),
                None => self.models.first().ok_or(ModelError::NoVisibleModels),
            }
        }
    }

    // Same semantics as the previous per-model projection, with no shared cache.
    fn legacy_projection(catalog: &dyn ModelCatalog) -> Vec<ModelInfo> {
        catalog
            .list_visible()
            .into_iter()
            .map(|model| model_info_from_catalog_model(model, catalog, &mut BTreeMap::new()))
            .collect()
    }

    #[test]
    fn catalog_projection_preserves_metadata_and_only_reads_each_provider_once() {
        let mut catalog = CountingCatalog::fixture(2, 32);
        // Preserve ordering/duplicates, slashes in model ids, and legacy missing metadata.
        catalog.models.push(catalog.models[0].clone());
        catalog.models.push(Model {
            slug: "legacy-model".to_string(),
            ..Model::default()
        });
        catalog.models.push(Model {
            slug: "missing/model".to_string(),
            ..Model::default()
        });
        let expected = legacy_projection(&catalog);
        catalog.reads.lock().unwrap().clear();
        assert_eq!(model_infos_from_catalog(&catalog), expected);
        assert_eq!(
            *catalog.reads.lock().unwrap(),
            BTreeMap::from([
                ("provider-0".to_string(), 1),
                ("provider-1".to_string(), 1),
                ("missing".to_string(), 1),
            ])
        );
    }

    #[test]
    fn catalog_projection_matches_canonical_wire_snapshot() {
        let catalog = CountingCatalog::fixture(1, 1);
        assert_eq!(
            serde_json::to_value(model_infos_from_catalog(&catalog)).unwrap(),
            serde_json::json!([{
                "slug": "provider-0/family/model-0", "displayName": "Model 0",
                "providerId": "provider-0", "modelId": "family/model-0",
                "provider": "openai_chat_completions", "contextWindow": 128000,
                "reasoningCapability": "unsupported", "availableThinkingLevels": ["off"],
                "inputModalities": ["text"], "maxTokens": 8192
            }])
        );
    }

    struct DefaultMetadataCatalog(Vec<Model>);

    impl ModelCatalog for DefaultMetadataCatalog {
        fn list_visible(&self) -> Vec<&Model> {
            self.0.iter().collect()
        }
        fn get(&self, slug: &str) -> Option<&Model> {
            self.0.iter().find(|model| model.slug == slug)
        }
        fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError> {
            match requested {
                Some(slug) => self.get(slug).ok_or_else(|| ModelError::ModelNotFound {
                    slug: slug.to_string(),
                }),
                None => self.0.first().ok_or(ModelError::NoVisibleModels),
            }
        }
    }

    #[test]
    fn catalog_projection_supports_default_trait_metadata() {
        let catalog = DefaultMetadataCatalog(CountingCatalog::fixture(2, 3).models);
        assert_eq!(
            model_infos_from_catalog(&catalog),
            legacy_projection(&catalog)
        );
    }

    #[test]
    fn catalog_projection_does_not_cache_between_requests() {
        let mut catalog = CountingCatalog::fixture(1, 1);
        let first = model_infos_from_catalog(&catalog);
        catalog
            .metadata
            .get_mut("provider-0")
            .unwrap()
            .get_mut("family/model-0")
            .unwrap()
            .max_tokens = Some(4096);
        let second = model_infos_from_catalog(&catalog);
        assert_ne!(first, second);
        assert_eq!(second, legacy_projection(&catalog));
        assert_eq!(
            model_infos_from_catalog(&CountingCatalog::fixture(0, 0)),
            Vec::<ModelInfo>::new()
        );
    }

    #[test]
    #[ignore = "manual local CPU benchmark, no network or model calls"]
    fn catalog_projection_large_directory_benchmark() {
        let catalog = CountingCatalog::fixture(20, 300);
        let started = Instant::now();
        let before = legacy_projection(&catalog);
        let legacy = started.elapsed();
        catalog.reads.lock().unwrap().clear();
        let started = Instant::now();
        let after = model_infos_from_catalog(&catalog);
        let optimized = started.elapsed();
        assert_eq!(after, before);
        assert_eq!(catalog.reads.lock().unwrap().values().sum::<usize>(), 20);
        eprintln!(
            "catalog projection 6000 models: old={legacy:?}, optimized={optimized:?}; full provider map copies 6000 -> 20"
        );
    }
}
