//! Builtin model catalog loading and resolution for core.
//!
//! Main focus:
//! - load the bundled preset list from disk-independent embedded assets
//! - load per-user and per-project model overrides from the filesystem
//! - convert raw `ModelPreset` values into runtime `Model` values
//! - provide the concrete builtin implementation of the shared `ModelCatalog` trait
//!
//! Design:
//! - catalog loading stays in `devo-core` because the embedded assets live here
//! - this module is the bridge between raw preset/config data and runtime model consumers
//! - models are sorted and materialized here so downstream code can work only with resolved `Model`
//! - precedence is: `<workspace>/.devo/models.json` > `~/.devo/models.json` > builtin
//!
//! Boundary:
//! - this module should not define the runtime model shape itself; that lives in `devo-protocol`
//! - serde compatibility for the raw preset file belongs in `model_preset.rs`
//! - execution logic should depend on `ModelCatalog` and `Model`, not on how this module reads JSON
//!
use std::path::{Path, PathBuf};

use crate::{Model, ModelCatalog, ModelError, ModelPreset};

const BUILTIN_MODELS_JSON: &str = include_str!("../models.json");

pub use crate::model_preset::default_base_instructions;

/// Filesystem-independent loader for the built-in model catalog bundled with the binary.
///
/// Use [`PresetModelCatalog::load_from_config`] to include user and project overrides.
/// Use [`PresetModelCatalog::load`] for the builtin-only variant (tests, doctor, etc.).
#[derive(Debug, Clone, Default)]
pub struct PresetModelCatalog {
    models: Vec<Model>,
    warnings: Vec<ModelCatalogWarning>,
}

/// Non-fatal filesystem catalog issue recorded while loading `models.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalogWarning {
    pub path: PathBuf,
    pub message: String,
}

impl PresetModelCatalog {
    /// Loads the built-in catalog only (no filesystem overrides).
    pub fn load() -> Result<Self, PresetModelCatalogError> {
        Ok(Self {
            models: load_builtin_models()?,
            warnings: Vec::new(),
        })
    }

    /// Loads the effective catalog from three layers. Precedence is:
    /// 1. `<workspace_root>/.devo/models.json` (project overrides)
    /// 2. `config_home/models.json` (user overrides)
    /// 3. built-in models (embedded fallback)
    ///
    /// Implementation loads from fallback to highest precedence so later
    /// layers can replace entries with the same slug.
    ///
    /// If the user file does not exist it is seeded from the builtin list so
    /// users can discover and customize the catalog.
    pub fn load_from_config(
        config_home: &Path,
        workspace_root: Option<&Path>,
    ) -> Result<Self, PresetModelCatalogError> {
        seed_user_models_file(config_home);

        let mut presets = load_builtin_model_presets()?;

        let mut warnings = Vec::new();
        merge_filesystem_model_presets(
            &mut presets,
            &mut warnings,
            &config_home.join("models.json"),
        );

        if let Some(workspace_root) = workspace_root {
            let project_path = workspace_root.join(".devo").join("models.json");
            merge_filesystem_model_presets(&mut presets, &mut warnings, &project_path);
        }

        presets.sort_by(|left, right| right.priority.cmp(&left.priority));
        Ok(Self {
            models: presets.into_iter().map(Model::from).collect(),
            warnings,
        })
    }

    /// Creates a catalog from an already-loaded model list.
    pub fn new(models: Vec<Model>) -> Self {
        Self {
            models,
            warnings: Vec::new(),
        }
    }

    /// Returns the loaded models by value.
    pub fn into_inner(self) -> Vec<Model> {
        self.models
    }

    /// Returns non-fatal warnings encountered while loading filesystem overrides.
    pub fn warnings(&self) -> &[ModelCatalogWarning] {
        &self.warnings
    }
}

impl ModelCatalog for PresetModelCatalog {
    fn list_visible(&self) -> Vec<&Model> {
        self.models.iter().collect()
    }

    fn get(&self, slug: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.slug == slug)
    }

    /// Resolves an explicit requested slug, or falls back to the first visible preset model.
    fn resolve_for_turn(&self, requested: Option<&str>) -> Result<&Model, ModelError> {
        if let Some(slug) = requested {
            return self.get(slug).ok_or_else(|| ModelError::ModelNotFound {
                slug: slug.to_string(),
            });
        }

        self.list_visible()
            .into_iter()
            .next()
            .ok_or(ModelError::NoVisibleModels)
    }
}

/// Loads the built-in raw model preset list bundled with the crate.
pub fn load_builtin_model_presets() -> Result<Vec<ModelPreset>, PresetModelCatalogError> {
    serde_json::from_str(BUILTIN_MODELS_JSON).map_err(Into::into)
}

/// Loads the built-in model list bundled with the crate.
pub fn load_builtin_models() -> Result<Vec<Model>, PresetModelCatalogError> {
    let mut presets = load_builtin_model_presets()?;
    presets.sort_by(|left, right| right.priority.cmp(&left.priority));
    Ok(presets.into_iter().map(Model::from).collect())
}

/// Reads model presets from a filesystem JSON path. Missing files return `None`;
/// invalid files return an error so callers can warn while continuing.
fn load_models_from_file(path: &Path) -> Result<Option<Vec<ModelPreset>>, ModelCatalogFileError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ModelCatalogFileError::Read(error)),
    };
    if contents.trim().is_empty() {
        return Ok(Some(Vec::new()));
    }
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(ModelCatalogFileError::Parse)
}

fn merge_filesystem_model_presets(
    presets: &mut Vec<ModelPreset>,
    warnings: &mut Vec<ModelCatalogWarning>,
    path: &Path,
) {
    match load_models_from_file(path) {
        Ok(Some(overrides)) => {
            *presets = merge_model_presets(std::mem::take(presets), overrides);
        }
        Ok(None) => {}
        Err(error) => {
            let message = error.to_string();
            tracing::warn!(
                path = %path.display(),
                error = %message,
                "skipping invalid model catalog override"
            );
            warnings.push(ModelCatalogWarning {
                path: path.to_path_buf(),
                message,
            });
        }
    }
}

/// Merges two model lists by slug. Entries from `overlay` replace matching
/// entries in `base`; entries with new slugs are appended.
fn merge_model_presets(mut base: Vec<ModelPreset>, overlay: Vec<ModelPreset>) -> Vec<ModelPreset> {
    for entry in overlay {
        match base.iter_mut().find(|m| m.slug == entry.slug) {
            Some(existing) => *existing = entry,
            None => base.push(entry),
        }
    }
    base
}

/// Copies the built-in `models.json` to the user config directory if no user
/// file exists yet.
fn seed_user_models_file(config_home: &Path) {
    let user_path = config_home.join("models.json");
    if user_path.exists() {
        return;
    }
    if let Some(parent) = user_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&user_path, BUILTIN_MODELS_JSON);
}

/// Errors produced while loading the builtin catalog.
#[derive(Debug, thiserror::Error)]
pub enum PresetModelCatalogError {
    /// Parsing the bundled JSON file failed.
    #[error("failed to parse builtin model catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
enum ModelCatalogFileError {
    #[error("failed to read model catalog: {0}")]
    Read(#[from] std::io::Error),
    #[error("failed to parse model catalog: {0}")]
    Parse(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pretty_assertions::assert_eq;

    use super::{
        PresetModelCatalog, default_base_instructions, load_builtin_model_presets,
        load_builtin_models, merge_model_presets,
    };
    use crate::{ModelCatalog, ModelPreset};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devo-{name}-{nanos}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn make_preset(slug: &str, display_name: &str, priority: i32) -> ModelPreset {
        ModelPreset {
            slug: slug.into(),
            display_name: display_name.into(),
            priority,
            ..ModelPreset::default()
        }
    }

    fn model_by_slug(models: &[crate::Model], slug: &str) -> crate::Model {
        models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
            .expect("model exists")
    }

    #[test]
    fn builtin_model_presets_load_from_bundled_json() {
        let presets = load_builtin_model_presets().expect("load builtin model presets");
        assert!(!presets.is_empty());
        assert_eq!(presets[0].slug, "qwen3-coder-next");
        assert!(
            presets[0]
                .base_instructions
                .as_ref()
                .is_some_and(|instructions| !instructions.is_empty())
        );
    }

    #[test]
    fn builtin_models_load_from_bundled_json() {
        let models = load_builtin_models().expect("load builtin models");
        assert!(!models.is_empty());
        assert_eq!(models[0].slug, "qwen3-coder-next");
        assert!(!models[0].base_instructions.is_empty());
    }

    #[test]
    fn builtin_catalog_resolves_visible_defaults() {
        let catalog = PresetModelCatalog::load().expect("load catalog");
        let model = catalog.resolve_for_turn(None).expect("resolve default");
        assert!(!model.slug.is_empty());
    }

    #[test]
    fn default_base_instructions_are_available() {
        assert!(!default_base_instructions().trim().is_empty());
    }

    #[test]
    fn merge_by_slug_overrides_existing() {
        let base = vec![make_preset("a", "Base A", 10)];
        let overlay = vec![make_preset("a", "Overlay A", 20)];
        let merged = merge_model_presets(base, overlay);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].display_name, "Overlay A");
        assert_eq!(merged[0].priority, 20);
    }

    #[test]
    fn merge_by_slug_appends_new() {
        let base = vec![make_preset("a", "A", 10)];
        let overlay = vec![make_preset("b", "B", 20)];
        let merged = merge_model_presets(base, overlay);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].slug, "a");
        assert_eq!(merged[1].slug, "b");
    }

    #[test]
    fn merge_empty_overlay_does_nothing() {
        let base = vec![make_preset("a", "A", 10)];
        let merged = merge_model_presets(base, Vec::new());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].display_name, "A");
    }

    #[test]
    fn load_from_config_returns_builtin_when_no_filesystem_files() {
        let root = unique_temp_dir("catalog-builtin-only");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");

        let catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");
        let models = catalog.into_inner();
        assert!(!models.is_empty());
        assert_eq!(models[0].slug, "qwen3-coder-next");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_seeds_user_file_when_missing() {
        let root = unique_temp_dir("catalog-seed");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");

        let user_file = home.join("models.json");
        assert!(!user_file.exists());

        let _catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");

        assert!(user_file.exists());
        let contents = std::fs::read_to_string(&user_file).expect("read");
        assert!(contents.contains("qwen3-coder-next"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_does_not_overwrite_existing_user_file() {
        let root = unique_temp_dir("catalog-no-overwrite");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");

        let user_file = home.join("models.json");
        std::fs::write(
            &user_file,
            "[{\"slug\":\"custom\",\"display_name\":\"Custom\"}]",
        )
        .expect("write");

        let catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");
        let models = catalog.into_inner();

        assert!(models.iter().any(|m| m.slug == "custom"));
        assert!(models.iter().any(|m| m.slug == "qwen3-coder-next"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_applies_user_model_token_overrides() {
        let root = unique_temp_dir("catalog-user-token-overrides");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");

        std::fs::write(
            home.join("models.json"),
            r#"[
                {
                    "slug": "qwen3-coder-next",
                    "display_name": "Custom Qwen",
                    "context_window": 123456,
                    "effective_context_window_percent": 77,
                    "max_tokens": 7654
                }
            ]"#,
        )
        .expect("write user models");

        let catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");
        let model = model_by_slug(&catalog.into_inner(), "qwen3-coder-next");

        assert_eq!(model.context_window, 123456);
        assert_eq!(model.effective_context_window_percent, Some(77));
        assert_eq!(model.max_tokens, Some(7654));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_missing_base_instructions_fall_back_to_default() {
        let root = unique_temp_dir("catalog-missing-base-instructions");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");

        std::fs::write(
            home.join("models.json"),
            r#"[
                {
                    "slug": "qwen3-coder-next",
                    "display_name": "Custom Qwen"
                }
            ]"#,
        )
        .expect("write user models");

        let catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");
        let model = model_by_slug(&catalog.into_inner(), "qwen3-coder-next");

        assert_eq!(model.display_name, "Custom Qwen");
        assert_eq!(model.base_instructions, default_base_instructions());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_project_overrides_user_by_slug() {
        let root = unique_temp_dir("catalog-project-wins");
        let home = root.join("home").join(".devo");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&home).expect("create home");
        std::fs::create_dir_all(workspace.join(".devo")).expect("create project");

        std::fs::write(
            home.join("models.json"),
            r#"[{"slug":"custom","display_name":"User","context_window":111,"effective_context_window_percent":66,"max_tokens":222}]"#,
        )
        .expect("write user models");
        std::fs::write(
            workspace.join(".devo").join("models.json"),
            r#"[{"slug":"custom","display_name":"Project","context_window":333,"effective_context_window_percent":88,"max_tokens":444}]"#,
        )
        .expect("write project models");

        let catalog = PresetModelCatalog::load_from_config(&home, Some(&workspace)).expect("load");
        let model = model_by_slug(&catalog.into_inner(), "custom");

        assert_eq!(model.display_name, "Project");
        assert_eq!(model.context_window, 333);
        assert_eq!(model.effective_context_window_percent, Some(88));
        assert_eq!(model.max_tokens, Some(444));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_uses_workspace_user_builtin_precedence_by_slug() {
        let root = unique_temp_dir("catalog-precedence");
        let home = root.join("home").join(".devo");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&home).expect("create home");
        std::fs::create_dir_all(workspace.join(".devo")).expect("create project");

        let user_models = home.join("models.json");
        let workspace_models = workspace.join(".devo").join("models.json");
        std::fs::write(
            &user_models,
            r#"[{"slug":"qwen3-coder-next","display_name":"User","context_window":111,"effective_context_window_percent":66,"max_tokens":222}]"#,
        )
        .expect("write user models");
        std::fs::write(
            &workspace_models,
            r#"[{"slug":"qwen3-coder-next","display_name":"Workspace","context_window":333,"effective_context_window_percent":88,"max_tokens":444}]"#,
        )
        .expect("write project models");

        let catalog = PresetModelCatalog::load_from_config(&home, Some(&workspace)).expect("load");
        let model = model_by_slug(&catalog.into_inner(), "qwen3-coder-next");

        assert_eq!(
            model,
            crate::Model::from(ModelPreset {
                slug: "qwen3-coder-next".into(),
                display_name: "Workspace".into(),
                context_window: 333,
                effective_context_window_percent: Some(88),
                input_modalities: vec![crate::InputModality::Text, crate::InputModality::Image],
                max_tokens: Some(444),
                ..ModelPreset::default()
            })
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_config_records_warning_and_continues_for_invalid_filesystem_catalog() {
        let root = unique_temp_dir("catalog-invalid-warning");
        let home = root.join("home").join(".devo");
        std::fs::create_dir_all(&home).expect("create home");
        let user_file = home.join("models.json");
        std::fs::write(&user_file, "{not valid json").expect("write invalid user models");

        let catalog =
            PresetModelCatalog::load_from_config(&home, /*workspace_root*/ None).expect("load");

        assert!(catalog.get("qwen3-coder-next").is_some());
        assert_eq!(catalog.warnings().len(), 1);
        assert_eq!(catalog.warnings()[0].path, user_file);
        assert!(
            catalog.warnings()[0]
                .message
                .contains("failed to parse model catalog")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn builtin_models_have_channel_fields() {
        let models = load_builtin_models().expect("load builtin models");
        let deepseek_models: Vec<_> = models
            .iter()
            .filter(|m| m.channel.as_deref() == Some("DeepSeek"))
            .collect();
        assert!(!deepseek_models.is_empty());
        assert!(deepseek_models.iter().any(|m| m.slug == "deepseek-v4-pro"));
    }
}
