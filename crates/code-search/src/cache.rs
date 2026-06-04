use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::files::FileManifestEntry;
use crate::types::{Chunk, CodeSearchError, ContentFilter};

const CACHE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedIndexPayloadV2 {
    pub cache_version: u32,
    pub root: PathBuf,
    pub content: ContentFilter,
    pub model_id: String,
    pub files: Vec<CachedFileRecord>,
}

impl CachedIndexPayloadV2 {
    pub fn new(
        root: PathBuf,
        content: ContentFilter,
        model_id: String,
        files: Vec<CachedFileRecord>,
    ) -> Self {
        Self {
            cache_version: CACHE_VERSION,
            root,
            content,
            model_id,
            files,
        }
    }

    pub fn is_valid_for(&self, root: &Path, content: ContentFilter, model_id: &str) -> bool {
        self.cache_version == CACHE_VERSION
            && self.root == root
            && self.content == content
            && self.model_id == model_id
            && self.is_internally_consistent()
    }

    fn is_loadable(&self) -> bool {
        self.cache_version == CACHE_VERSION && self.is_internally_consistent()
    }

    fn is_internally_consistent(&self) -> bool {
        self.files.iter().all(CachedFileRecord::is_consistent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedFileRecord {
    pub manifest: FileManifestEntry,
    pub content_hash: String,
    pub chunks: Vec<Chunk>,
    pub embeddings: Vec<Vec<f32>>,
}

impl CachedFileRecord {
    pub fn new(
        manifest: FileManifestEntry,
        content_hash: String,
        chunks: Vec<Chunk>,
        embeddings: Vec<Vec<f32>>,
    ) -> Self {
        Self {
            manifest,
            content_hash,
            chunks,
            embeddings,
        }
    }

    pub fn can_reuse_for(&self, manifest: &FileManifestEntry) -> bool {
        &self.manifest == manifest && self.is_consistent()
    }

    fn is_consistent(&self) -> bool {
        self.chunks.len() == self.embeddings.len()
    }
}

pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("devo")
        .join("code-search")
        .join("indexes")
}

pub fn cache_file_path(
    cache_dir: &Path,
    root: &Path,
    content: ContentFilter,
    model_id: &str,
) -> PathBuf {
    cache_dir.join(format!("{}.json", cache_key(root, content, model_id)))
}

pub fn load_payload(path: &Path) -> Option<CachedIndexPayloadV2> {
    let bytes = std::fs::read(path).ok()?;
    let payload = serde_json::from_slice::<CachedIndexPayloadV2>(&bytes).ok()?;
    payload.is_loadable().then_some(payload)
}

pub fn save_payload(path: &Path, payload: &CachedIndexPayloadV2) -> Result<(), CodeSearchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes =
        serde_json::to_vec(payload).map_err(|error| CodeSearchError::Io(error.to_string()))?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn content_hash(text: &str) -> String {
    hex_sha256(text.as_bytes())
}

fn cache_key(root: &Path, content: ContentFilter, model_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(format!("{content:?}").as_bytes());
    hasher.update(model_id.as_bytes());
    hex_digest(hasher)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}

fn hex_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: v2 cache payload validity depends on model, content filter, and record consistency.
    #[test]
    fn cached_payload_validates_header_and_records() {
        let record = CachedFileRecord::new(
            FileManifestEntry {
                path: PathBuf::from("src/lib.rs"),
                size: 10,
                modified_unix_nanos: 1,
            },
            content_hash("fn parse() {}"),
            vec![Chunk {
                content: "fn parse() {}".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            }],
            vec![vec![1.0]],
        );
        let payload = CachedIndexPayloadV2::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "model-a".to_string(),
            vec![record],
        );

        let validity = vec![
            payload.is_valid_for(Path::new("/repo"), ContentFilter::Code, "model-a"),
            payload.is_valid_for(Path::new("/repo"), ContentFilter::Code, "model-b"),
        ];

        assert_eq!(validity, vec![true, false]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: stale v1 and malformed cache files are treated as disposable cache misses.
    #[test]
    fn load_payload_rejects_v1_and_malformed_cache_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let v1_path = temp.path().join("v1.json");
        let malformed_path = temp.path().join("malformed.json");
        fs::write(
            &v1_path,
            r#"{"cache_version":1,"root":"/repo","content":"code","model_id":"test","manifest":[],"chunks":[],"embeddings":[]}"#,
        )
        .expect("write v1");
        fs::write(&malformed_path, b"{").expect("write malformed");

        let loaded = vec![load_payload(&v1_path), load_payload(&malformed_path)];

        assert_eq!(loaded, vec![None, None]);
    }
}
