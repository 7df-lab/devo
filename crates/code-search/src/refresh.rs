use std::collections::HashMap;
use std::path::Path;

use crate::cache::{CachedFileRecord, CachedIndexPayloadV2, content_hash};
use crate::chunking::chunk_file;
use crate::dense::EmbeddingProvider;
use crate::files::{FileEntry, FileManifestEntry, read_indexable_text};
use crate::types::{Chunk, CodeSearchError, ContentFilter};

pub struct IndexRefresh;

impl IndexRefresh {
    pub fn refresh(
        root: &Path,
        content: ContentFilter,
        files: Vec<FileEntry>,
        previous_payload: Option<CachedIndexPayloadV2>,
        provider: &dyn EmbeddingProvider,
    ) -> Result<RefreshOutcome, CodeSearchError> {
        let mut previous_records = previous_payload
            .filter(|payload| payload.is_valid_for(root, content, provider.model_id()))
            .map(|payload| {
                payload
                    .files
                    .into_iter()
                    .map(|record| (record.manifest.path.clone(), record))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        let mut records = Vec::new();
        let mut pending_records = Vec::new();
        let mut reused_files = 0;

        for file in files {
            if let Some(record) = previous_records.remove(&file.relative_path)
                && record.can_reuse_for(&file.manifest)
            {
                reused_files += 1;
                records.push(record);
                continue;
            }
            pending_records.push(PendingFileRecord::read(file)?);
        }

        let deleted_files = previous_records.len();
        let reembedded_files = pending_records.len();
        let texts = pending_records
            .iter()
            .flat_map(|record| record.chunks.iter().map(|chunk| chunk.content.clone()))
            .collect::<Vec<_>>();
        let embeddings = if texts.is_empty() {
            Vec::new()
        } else {
            provider.embed(&texts)?
        };
        if embeddings.len() != texts.len() {
            return Err(CodeSearchError::Index(format!(
                "embedding provider returned {} vectors for {} chunks",
                embeddings.len(),
                texts.len()
            )));
        }

        let mut embedding_cursor = 0;
        for pending in pending_records {
            let next_cursor = embedding_cursor + pending.chunks.len();
            let file_embeddings = embeddings[embedding_cursor..next_cursor].to_vec();
            embedding_cursor = next_cursor;
            records.push(CachedFileRecord::new(
                pending.manifest,
                pending.content_hash,
                pending.chunks,
                file_embeddings,
            ));
        }

        records.sort_by(|left, right| left.manifest.path.cmp(&right.manifest.path));
        let payload = CachedIndexPayloadV2::new(
            root.to_path_buf(),
            content,
            provider.model_id().to_string(),
            records,
        );
        Ok(RefreshOutcome {
            payload,
            reused_files,
            reembedded_files,
            deleted_files,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshOutcome {
    pub payload: CachedIndexPayloadV2,
    pub reused_files: usize,
    pub reembedded_files: usize,
    pub deleted_files: usize,
}

struct PendingFileRecord {
    manifest: FileManifestEntry,
    content_hash: String,
    chunks: Vec<Chunk>,
}

impl PendingFileRecord {
    fn read(file: FileEntry) -> Result<Self, CodeSearchError> {
        let (content_hash_value, chunks) = match read_indexable_text(&file.absolute_path) {
            Ok(Some(text)) => (
                content_hash(&text),
                chunk_file(&file.relative_path, &file.language, &text),
            ),
            Ok(None) => (content_hash(""), Vec::new()),
            Err(_) => (content_hash("unreadable"), Vec::new()),
        };
        Ok(Self {
            manifest: file.manifest,
            content_hash: content_hash_value,
            chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use pretty_assertions::assert_eq;

    use crate::dense::{EmbeddingProvider, HashEmbeddingProvider};
    use crate::files::discover_files;
    use crate::index::SearchIndex;

    use super::*;

    #[derive(Debug)]
    struct CountingProvider {
        inner: HashEmbeddingProvider,
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl CountingProvider {
        fn new() -> Self {
            Self {
                inner: HashEmbeddingProvider::new("test", 16),
                batches: Mutex::new(Vec::new()),
            }
        }

        fn batches(&self) -> Vec<Vec<String>> {
            self.batches.lock().expect("batches").clone()
        }
    }

    impl EmbeddingProvider for CountingProvider {
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }

        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
            self.batches.lock().expect("batches").push(texts.to_vec());
            self.inner.embed(texts)
        }
    }

    fn refresh(
        root: &Path,
        previous_payload: Option<CachedIndexPayloadV2>,
        provider: &CountingProvider,
    ) -> RefreshOutcome {
        let files = discover_files(root, ContentFilter::Code).expect("files");
        IndexRefresh::refresh(root, ContentFilter::Code, files, previous_payload, provider)
            .expect("refresh")
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: unchanged files reuse cached chunks and embeddings without another embedding call.
    #[test]
    fn unchanged_files_reuse_cached_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("lib.rs"), "pub fn parse_input() {}\n").expect("write");
        let provider = CountingProvider::new();

        let first = refresh(temp.path(), None, &provider);
        let second = refresh(temp.path(), Some(first.payload.clone()), &provider);

        assert_eq!(second.payload, first.payload);
        assert_eq!(second.reused_files, 1);
        assert_eq!(second.reembedded_files, 0);
        assert_eq!(provider.batches().len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: changing one file re-embeds only that file while reusing unchanged file records.
    #[test]
    fn changed_file_reembeds_only_that_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.rs"), "pub fn alpha() {}\n").expect("write a");
        fs::write(temp.path().join("b.rs"), "pub fn beta() {}\n").expect("write b");
        let provider = CountingProvider::new();
        let first = refresh(temp.path(), None, &provider);
        fs::write(
            temp.path().join("b.rs"),
            "pub fn beta_changed_with_longer_body() {}\n",
        )
        .expect("rewrite b");

        let second = refresh(temp.path(), Some(first.payload), &provider);

        assert_eq!(second.reused_files, 1);
        assert_eq!(second.reembedded_files, 1);
        assert_eq!(second.deleted_files, 0);
        assert_eq!(provider.batches().len(), 2);
        assert_eq!(provider.batches()[1].len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: adding and deleting files updates the v2 payload and flattened search index.
    #[test]
    fn added_and_deleted_files_update_flattened_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.rs"), "pub fn alpha() {}\n").expect("write a");
        fs::write(temp.path().join("b.rs"), "pub fn beta() {}\n").expect("write b");
        let provider = CountingProvider::new();
        let first = refresh(temp.path(), None, &provider);
        fs::remove_file(temp.path().join("a.rs")).expect("remove a");
        fs::write(temp.path().join("c.rs"), "pub fn gamma() {}\n").expect("write c");

        let second = refresh(temp.path(), Some(first.payload), &provider);
        let index = SearchIndex::from_payload(second.payload.clone()).expect("index");
        let payload_paths = second
            .payload
            .files
            .iter()
            .map(|record| record.manifest.path.clone())
            .collect::<Vec<_>>();
        let flattened_paths = (0..index.stats().total_chunks)
            .filter_map(|idx| index.chunk(idx).map(|chunk| chunk.file_path.clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            payload_paths,
            vec![PathBuf::from("b.rs"), PathBuf::from("c.rs")]
        );
        assert_eq!(
            flattened_paths,
            vec![PathBuf::from("b.rs"), PathBuf::from("c.rs")]
        );
        assert_eq!(second.deleted_files, 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: empty files are cached as zero-chunk file records until their manifest changes.
    #[test]
    fn empty_files_create_reusable_zero_chunk_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("empty.rs"), "   \n").expect("write");
        let provider = CountingProvider::new();

        let first = refresh(temp.path(), None, &provider);
        let second = refresh(temp.path(), Some(first.payload.clone()), &provider);

        assert_eq!(first.payload.files[0].chunks, Vec::<Chunk>::new());
        assert_eq!(second.reused_files, 1);
        assert_eq!(provider.batches(), Vec::<Vec<String>>::new());
    }
}
