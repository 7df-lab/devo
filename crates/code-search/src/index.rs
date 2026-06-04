use std::path::{Path, PathBuf};

use bm25::{Document, SearchEngine, SearchEngineBuilder, Tokenizer};

use crate::cache::CachedIndexPayloadV2;
#[cfg(test)]
use crate::dense::EmbeddingProvider;
use crate::dense::cosine_similarity;
#[cfg(test)]
use crate::files::FileEntry;
use crate::files::FileManifestEntry;
#[cfg(test)]
use crate::refresh::IndexRefresh;
use crate::tokens::{enrich_for_bm25, split_identifier_tokens};
use crate::types::{
    Chunk, CodeSearchError, ContentFilter, IndexStats, SearchFilters, SearchResult,
};

#[derive(Debug, Clone, Copy)]
pub struct CodeTokenizer;

impl Tokenizer for CodeTokenizer {
    fn tokenize(&self, input_text: &str) -> Vec<String> {
        split_identifier_tokens(input_text)
    }
}

pub struct SearchIndex {
    root: PathBuf,
    content: ContentFilter,
    manifest: Vec<FileManifestEntry>,
    chunks: Vec<Chunk>,
    embeddings: Vec<Vec<f32>>,
    bm25: SearchEngine<usize, u32, CodeTokenizer>,
    stats: IndexStats,
}

impl SearchIndex {
    #[cfg(test)]
    pub fn build(
        root: PathBuf,
        content: ContentFilter,
        files: &[FileEntry],
        provider: &dyn EmbeddingProvider,
    ) -> Result<Self, CodeSearchError> {
        let outcome =
            IndexRefresh::refresh(root.as_path(), content, files.to_vec(), None, provider)?;
        Self::from_payload(outcome.payload)
    }

    pub fn from_payload(payload: CachedIndexPayloadV2) -> Result<Self, CodeSearchError> {
        let indexed_files = payload.files.len();
        let mut manifest = Vec::new();
        let mut chunks = Vec::new();
        let mut embeddings = Vec::new();
        for record in payload.files {
            if record.chunks.len() != record.embeddings.len() {
                return Err(CodeSearchError::Index(
                    "cached chunk and embedding counts do not match".to_string(),
                ));
            }
            manifest.push(record.manifest);
            chunks.extend(record.chunks);
            embeddings.extend(record.embeddings);
        }
        Self::from_parts(
            payload.root,
            payload.content,
            manifest,
            chunks,
            embeddings,
            indexed_files,
        )
    }

    pub fn stats(&self) -> IndexStats {
        self.stats.clone()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn content(&self) -> ContentFilter {
        self.content
    }

    pub fn manifest_matches(&self, manifest: &[FileManifestEntry]) -> bool {
        self.manifest == manifest
    }

    pub fn chunk(&self, chunk_id: usize) -> Option<&Chunk> {
        self.chunks.get(chunk_id)
    }

    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<(usize, f32)> {
        let mut scores = self
            .embeddings
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                self.chunks
                    .get(*idx)
                    .is_some_and(|chunk| filters.allows(chunk))
            })
            .map(|(idx, embedding)| (idx, cosine_similarity(query_embedding, embedding)))
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();
        scores.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        scores.truncate(limit);
        scores
    }

    pub fn sparse_search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<(usize, f32)> {
        self.bm25
            .search(query, limit)
            .into_iter()
            .filter(|result| result.score > 0.0)
            .filter(|result| {
                self.chunks
                    .get(result.document.id)
                    .is_some_and(|chunk| filters.allows(chunk))
            })
            .map(|result| (result.document.id, result.score))
            .collect()
    }

    pub fn related_by_embedding(
        &self,
        source_idx: usize,
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<SearchResult> {
        let Some(source_chunk) = self.chunks.get(source_idx) else {
            return Vec::new();
        };
        let Some(source_embedding) = self.embeddings.get(source_idx) else {
            return Vec::new();
        };
        let mut results = self
            .embeddings
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != source_idx)
            .filter_map(|(idx, embedding)| {
                let chunk = self.chunks.get(idx)?;
                if chunk.language != source_chunk.language || !filters.allows(chunk) {
                    return None;
                }
                let score = cosine_similarity(source_embedding, embedding);
                (score > 0.0).then(|| SearchResult {
                    score,
                    chunk: chunk.clone(),
                })
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.chunk.location().cmp(&right.chunk.location()))
        });
        results.truncate(limit);
        results
    }

    pub fn find_source_chunk(&self, file_path: &Path, line: usize) -> Option<usize> {
        let normalized = file_path.to_string_lossy().replace('\\', "/");
        self.chunks.iter().position(|chunk| {
            chunk.file_path.to_string_lossy().replace('\\', "/") == normalized
                && chunk.start_line <= line
                && line <= chunk.end_line
        })
    }

    fn from_parts(
        root: PathBuf,
        content: ContentFilter,
        manifest: Vec<FileManifestEntry>,
        chunks: Vec<Chunk>,
        embeddings: Vec<Vec<f32>>,
        indexed_files: usize,
    ) -> Result<Self, CodeSearchError> {
        if chunks.len() != embeddings.len() {
            return Err(CodeSearchError::Index(
                "cached chunk and embedding counts do not match".to_string(),
            ));
        }
        let bm25 = build_bm25(&chunks);
        let stats = IndexStats {
            indexed_files,
            total_chunks: chunks.len(),
        };
        Ok(Self {
            root,
            content,
            manifest,
            chunks,
            embeddings,
            bm25,
            stats,
        })
    }
}

fn build_bm25(chunks: &[Chunk]) -> SearchEngine<usize, u32, CodeTokenizer> {
    let documents = chunks
        .iter()
        .enumerate()
        .map(|(idx, chunk)| Document::new(idx, enrich_for_bm25(chunk)))
        .collect::<Vec<_>>();
    SearchEngineBuilder::<usize, u32, CodeTokenizer>::with_tokenizer_and_documents(
        CodeTokenizer,
        documents,
    )
    .build()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    use crate::dense::HashEmbeddingProvider;
    use crate::files::discover_files;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: index construction produces chunks, dense vectors, and searchable BM25 state.
    #[test]
    fn build_index_populates_sparse_and_dense_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("lib.rs"),
            "pub fn parse_input() {}\npub fn render_output() {}\n",
        )
        .expect("write");
        let files = discover_files(temp.path(), ContentFilter::Code).expect("files");
        let provider = Arc::new(HashEmbeddingProvider::new("test", 16));
        let index = SearchIndex::build(
            temp.path().to_path_buf(),
            ContentFilter::Code,
            &files,
            provider.as_ref(),
        )
        .expect("index");

        let sparse = index.sparse_search("parse input", 5, &SearchFilters::empty());

        assert_eq!(index.stats().indexed_files, 1);
        assert_eq!(index.stats().total_chunks, 1);
        assert_eq!(sparse.len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: find-related source lookup uses relative paths and 1-indexed line spans.
    #[test]
    fn source_chunk_lookup_matches_line_range() {
        let chunk = Chunk {
            content: "fn parse() {}".to_string(),
            file_path: PathBuf::from("src/lib.rs"),
            start_line: 10,
            end_line: 12,
            language: "rust".to_string(),
        };
        let payload = CachedIndexPayloadV2::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "test".to_string(),
            vec![crate::cache::CachedFileRecord::new(
                FileManifestEntry {
                    path: PathBuf::from("src/lib.rs"),
                    size: 10,
                    modified_unix_nanos: 1,
                },
                crate::cache::content_hash("fn parse() {}"),
                vec![chunk],
                vec![vec![1.0]],
            )],
        );
        let index = SearchIndex::from_payload(payload).expect("index");

        assert_eq!(
            index.find_source_chunk(Path::new("src/lib.rs"), 11),
            Some(0)
        );
        assert_eq!(index.find_source_chunk(Path::new("src/lib.rs"), 13), None);
    }
}
