//! Dense embedding providers and vector math.
//!
//! Production search uses model2vec with the Semble-compatible
//! `minishlab/potion-code-16M` model cached under the local Devo model
//! directory. Missing model files are fetched on first use; load/download
//! failures become typed `ModelUnavailable` errors so the tool can report a
//! recoverable cache/model problem instead of panicking. Tests use a deterministic
//! hash provider to exercise indexing without network or model dependencies.

use std::path::PathBuf;
use std::sync::Mutex;

use hf_hub::HFClientSync;
use model2vec::model::Model2Vec;
use sha2::{Digest, Sha256};

use crate::types::CodeSearchError;

const DEFAULT_MODEL_OWNER: &str = "minishlab";
const DEFAULT_MODEL_NAME: &str = "potion-code-16M";
const DEFAULT_MODEL_ID: &str = "minishlab/potion-code-16M";
const DEVO_HOME_ENV: &str = "DEVO_HOME";
const LOCAL_MODELS_DIR: &str = "local-models";
const MODEL_FILES: [&str; 3] = ["tokenizer.json", "model.safetensors", "config.json"];

#[derive(Debug)]
struct DefaultModelCacheInputs {
    devo_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    temp_dir: PathBuf,
}

/// Embeds code-search documents and queries into dense vectors.
///
/// Implementations must return exactly one vector for each input text, keep vector
/// dimensionality stable for the lifetime of a provider, and return normalized or
/// otherwise cosine-compatible vectors.
pub trait EmbeddingProvider: Send + Sync {
    /// Stable model identity used for cache invalidation.
    fn model_id(&self) -> &str;
    /// Embeds each supplied text into one dense vector.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError>;
}

/// Lazy model2vec provider for production dense retrieval.
///
/// The model is loaded behind a mutex on first embedding call. Keeping it lazy
/// avoids downloading or deserializing model files for validation paths that
/// return before indexing.
pub struct Model2VecEmbeddingProvider {
    model_id: String,
    model_dir: PathBuf,
    model: Mutex<Option<Model2Vec>>,
}

impl Model2VecEmbeddingProvider {
    /// Creates a provider that uses Devo's default local model cache.
    pub fn default_cached() -> Self {
        let model_dir = default_model_cache_dir();
        Self {
            model_id: DEFAULT_MODEL_ID.to_string(),
            model_dir,
            model: Mutex::new(None),
        }
    }

    /// Ensures required model files exist and loads model2vec state.
    ///
    /// `normalize_from_config` is left to the model config so the provider stays
    /// aligned with upstream model metadata.
    fn load_model(&self) -> Result<Model2Vec, CodeSearchError> {
        ensure_model_files(&self.model_dir)?;
        let normalize_from_config = None;
        let subdir = None;
        Model2Vec::from_pretrained(&self.model_dir, normalize_from_config, subdir)
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))
    }
}

impl Default for Model2VecEmbeddingProvider {
    fn default() -> Self {
        Self::default_cached()
    }
}

impl EmbeddingProvider for Model2VecEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self
            .model
            .lock()
            .map_err(|_| CodeSearchError::Index("model lock poisoned".to_string()))?;
        if guard.is_none() {
            *guard = Some(self.load_model()?);
        }
        let model = guard
            .as_ref()
            .ok_or_else(|| CodeSearchError::ModelUnavailable("model failed to load".to_string()))?;
        let matrix = model
            .encode(texts)
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;
        Ok(matrix.rows().into_iter().map(|row| row.to_vec()).collect())
    }
}

/// Deterministic embedding provider used by tests.
///
/// Hash embeddings are not semantically meaningful, but they preserve the core
/// invariants the index relies on: stable dimensions, one row per text, and
/// cosine-compatible normalization.
#[derive(Debug)]
pub struct HashEmbeddingProvider {
    model_id: String,
    dimensions: usize,
}

impl HashEmbeddingProvider {
    /// Creates a deterministic provider with a fixed vector dimension.
    pub fn new(model_id: impl Into<String>, dimensions: usize) -> Self {
        Self {
            model_id: model_id.into(),
            dimensions,
        }
    }
}

impl EmbeddingProvider for HashEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
        Ok(texts
            .iter()
            .map(|text| hash_embedding(text, self.dimensions))
            .collect())
    }
}

/// Computes cosine similarity, returning zero for incompatible vectors.
///
/// Returning zero keeps retrieval robust when a malformed cache or provider bug
/// slips past earlier validation; callers then simply get no positive semantic
/// hit for that pair.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (left_value, right_value) in left.iter().zip(right) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator <= f32::EPSILON {
        0.0
    } else {
        dot / denominator
    }
}

/// Downloads any missing model files into the local cache directory.
///
/// The check is all-or-download so a partially populated cache is repaired on
/// demand. If the final files are still missing, callers receive a typed model
/// error that can be shown as a recoverable tool failure.
fn ensure_model_files(model_dir: &PathBuf) -> Result<(), CodeSearchError> {
    if MODEL_FILES
        .iter()
        .all(|file| model_dir.join(file).is_file())
    {
        return Ok(());
    }
    std::fs::create_dir_all(model_dir)?;
    let client = HFClientSync::new()
        .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;
    let repo = client.model(DEFAULT_MODEL_OWNER, DEFAULT_MODEL_NAME);
    for file in MODEL_FILES {
        repo.download_file()
            .filename(file.to_string())
            .local_dir(model_dir.clone())
            .send()
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;
    }
    if MODEL_FILES
        .iter()
        .all(|file| model_dir.join(file).is_file())
    {
        Ok(())
    } else {
        Err(CodeSearchError::ModelUnavailable(format!(
            "model files for {DEFAULT_MODEL_ID} were not found in {} after download",
            model_dir.display()
        )))
    }
}

/// Computes the on-disk directory for the default model.
fn default_model_cache_dir() -> PathBuf {
    default_model_cache_dir_from_inputs(DefaultModelCacheInputs {
        devo_home: std::env::var_os(DEVO_HOME_ENV).map(PathBuf::from),
        home_dir: dirs::home_dir(),
        temp_dir: std::env::temp_dir(),
    })
}

fn default_model_cache_dir_from_inputs(inputs: DefaultModelCacheInputs) -> PathBuf {
    let base_dir = inputs
        .devo_home
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| inputs.home_dir.map(|home_dir| home_dir.join(".devo")))
        .unwrap_or_else(|| inputs.temp_dir.join(".devo"));
    base_dir
        .join(LOCAL_MODELS_DIR)
        .join(DEFAULT_MODEL_ID.replace('/', "--"))
}

/// Produces deterministic normalized bag-of-token vectors for tests.
fn hash_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let dimensions = dimensions.max(1);
    let mut vector = vec![0.0; dimensions];
    for token in text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let mut hasher = Sha256::new();
        hasher.update(token.to_lowercase().as_bytes());
        let digest = hasher.finalize();
        let index = u64::from_le_bytes([
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
        ]) as usize
            % dimensions;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: test embedding provider returns one stable vector per input.
    #[test]
    fn hash_embedding_provider_is_deterministic() {
        let provider = HashEmbeddingProvider::new("test", 8);
        let texts = vec!["parse input".to_string(), "parse input".to_string()];
        let embeddings = provider.embed(&texts).expect("embed");

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], embeddings[1]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: dense ranking similarity is cosine-compatible.
    #[test]
    fn cosine_similarity_scores_identical_vectors_highest() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn default_model_cache_dir_uses_devo_home_when_set() {
        let devo_home = PathBuf::from("devo-home");
        let inputs = DefaultModelCacheInputs {
            devo_home: Some(devo_home.clone()),
            home_dir: Some(PathBuf::from("home")),
            temp_dir: PathBuf::from("temp"),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            devo_home
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }

    #[test]
    fn default_model_cache_dir_uses_home_when_devo_home_is_absent() {
        let home_dir = PathBuf::from("home");
        let inputs = DefaultModelCacheInputs {
            devo_home: None,
            home_dir: Some(home_dir.clone()),
            temp_dir: PathBuf::from("temp"),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            home_dir
                .join(".devo")
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }

    #[test]
    fn default_model_cache_dir_treats_empty_devo_home_as_absent() {
        let home_dir = PathBuf::from("home");
        let inputs = DefaultModelCacheInputs {
            devo_home: Some(PathBuf::new()),
            home_dir: Some(home_dir.clone()),
            temp_dir: PathBuf::from("temp"),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            home_dir
                .join(".devo")
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }

    #[test]
    fn default_model_cache_dir_falls_back_to_temp_when_home_is_absent() {
        let temp_dir = PathBuf::from("temp");
        let inputs = DefaultModelCacheInputs {
            devo_home: None,
            home_dir: None,
            temp_dir: temp_dir.clone(),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            temp_dir
                .join(".devo")
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }
}
