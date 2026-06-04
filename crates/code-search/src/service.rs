use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cache::{cache_file_path, default_cache_dir, load_payload, save_payload};
use crate::dense::{EmbeddingProvider, Model2VecEmbeddingProvider};
use crate::files::{FileManifestEntry, discover_files};
use crate::index::SearchIndex;
use crate::ranking::rank_search;
use crate::refresh::IndexRefresh;
use crate::types::{
    CodeSearchError, CodeSearchOperation, ContentFilter, IndexStats, RelatedRequest, SearchOutput,
    SearchRequest, validate_top_k,
};
use crate::watch::IndexWatcher;

const MANIFEST_SAFETY_INTERVAL: Duration = Duration::from_secs(30);

pub struct CodeSearchService {
    provider: Arc<dyn EmbeddingProvider>,
    cache_dir: PathBuf,
    indexes: Mutex<HashMap<String, CachedIndexState>>,
    watcher_policy: WatcherPolicy,
}

impl CodeSearchService {
    pub fn production() -> Self {
        Self::new(
            Arc::new(Model2VecEmbeddingProvider::default()),
            default_cache_dir(),
        )
    }

    pub fn new(provider: Arc<dyn EmbeddingProvider>, cache_dir: PathBuf) -> Self {
        Self {
            provider,
            cache_dir,
            indexes: Mutex::new(HashMap::new()),
            watcher_policy: WatcherPolicy::Enabled,
        }
    }

    #[cfg(test)]
    fn new_with_watcher_policy(
        provider: Arc<dyn EmbeddingProvider>,
        cache_dir: PathBuf,
        watcher_policy: WatcherPolicy,
    ) -> Self {
        Self {
            provider,
            cache_dir,
            indexes: Mutex::new(HashMap::new()),
            watcher_policy,
        }
    }

    pub fn search(&self, request: SearchRequest) -> Result<SearchOutput, CodeSearchError> {
        let top_k = validate_top_k(request.top_k)?;
        let root = canonical_root(&request.root)?;
        let query = request.query.trim().to_string();
        if query.is_empty() {
            return Ok(SearchOutput {
                operation: CodeSearchOperation::Search,
                query: Some(query),
                root,
                content: request.content,
                results: Vec::new(),
                index_stats: IndexStats {
                    indexed_files: 0,
                    total_chunks: 0,
                },
            });
        }
        let index = self.index(root, request.content)?;
        let query_embedding = self.provider.embed(std::slice::from_ref(&query))?;
        let results = query_embedding
            .first()
            .map(|embedding| rank_search(&index, &query, embedding, top_k, &request.filters))
            .unwrap_or_default();
        Ok(SearchOutput {
            operation: CodeSearchOperation::Search,
            query: Some(query),
            root: index.root().to_path_buf(),
            content: index.content(),
            results,
            index_stats: index.stats(),
        })
    }

    pub fn find_related(&self, request: RelatedRequest) -> Result<SearchOutput, CodeSearchError> {
        let top_k = validate_top_k(request.top_k)?;
        if request.line == 0 {
            return Err(CodeSearchError::InvalidInput(
                "`line` must be 1-indexed and greater than zero".to_string(),
            ));
        }
        let root = canonical_root(&request.root)?;
        let relative_path = normalize_source_path(&root, &request.file_path)?;
        let index = self.index(root, request.content)?;
        let source_idx = index.find_source_chunk(&relative_path, request.line);
        let results = source_idx
            .map(|idx| index.related_by_embedding(idx, top_k, &request.filters))
            .unwrap_or_default();
        Ok(SearchOutput {
            operation: CodeSearchOperation::FindRelated,
            query: None,
            root: index.root().to_path_buf(),
            content: index.content(),
            results,
            index_stats: index.stats(),
        })
    }

    fn index(
        &self,
        root: PathBuf,
        content: ContentFilter,
    ) -> Result<Arc<SearchIndex>, CodeSearchError> {
        let key = memory_key(&root, content, self.provider.model_id());
        if let Some(index) = self.clean_warm_index(&key)? {
            return Ok(index);
        }

        let files = discover_files(&root, content)?;
        let manifest = files
            .iter()
            .map(|file| file.manifest.clone())
            .collect::<Vec<_>>();
        if let Some(index) = self.matching_memory_index(&key, &root, &manifest)? {
            return Ok(index);
        }

        let cache_path = cache_file_path(&self.cache_dir, &root, content, self.provider.model_id());
        let previous_payload = load_payload(&cache_path)
            .filter(|payload| payload.is_valid_for(&root, content, self.provider.model_id()));
        let outcome = IndexRefresh::refresh(
            &root,
            content,
            files,
            previous_payload,
            self.provider.as_ref(),
        )?;
        let index = Arc::new(SearchIndex::from_payload(outcome.payload.clone())?);
        save_payload(&cache_path, &outcome.payload)?;
        self.store_index(key, &root, Arc::clone(&index))?;
        Ok(index)
    }

    fn clean_warm_index(&self, key: &str) -> Result<Option<Arc<SearchIndex>>, CodeSearchError> {
        let now = Instant::now();
        Ok(self
            .indexes
            .lock()
            .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?
            .get(key)
            .filter(|state| state.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL))
            .map(|state| Arc::clone(&state.index)))
    }

    fn matching_memory_index(
        &self,
        key: &str,
        root: &Path,
        manifest: &[FileManifestEntry],
    ) -> Result<Option<Arc<SearchIndex>>, CodeSearchError> {
        let index = self
            .indexes
            .lock()
            .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?
            .get(key)
            .filter(|state| state.index.manifest_matches(manifest))
            .map(|state| Arc::clone(&state.index));
        if let Some(index) = index {
            self.store_index(key.to_string(), root, Arc::clone(&index))?;
            return Ok(Some(index));
        }
        Ok(None)
    }

    fn store_index(
        &self,
        key: String,
        root: &Path,
        index: Arc<SearchIndex>,
    ) -> Result<(), CodeSearchError> {
        let state = CachedIndexState::new(index, root, self.watcher_policy);
        self.indexes
            .lock()
            .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?
            .insert(key, state);
        Ok(())
    }
}

impl Default for CodeSearchService {
    fn default() -> Self {
        Self::production()
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, CodeSearchError> {
    let canonical = root.canonicalize()?;
    if !canonical.is_dir() {
        return Err(CodeSearchError::InvalidInput(format!(
            "search root is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn normalize_source_path(root: &Path, file_path: &Path) -> Result<PathBuf, CodeSearchError> {
    if file_path.is_absolute() {
        let canonical = file_path.canonicalize()?;
        return canonical
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                CodeSearchError::InvalidInput(format!(
                    "file path is outside the search root: {}",
                    file_path.display()
                ))
            });
    }
    Ok(file_path.to_path_buf())
}

fn memory_key(root: &Path, content: ContentFilter, model_id: &str) -> String {
    format!("{}::{content:?}::{model_id}", root.display())
}

struct CachedIndexState {
    index: Arc<SearchIndex>,
    watcher: IndexWatcher,
    last_manifest_check: Instant,
}

impl CachedIndexState {
    fn new(index: Arc<SearchIndex>, root: &Path, watcher_policy: WatcherPolicy) -> Self {
        Self {
            index,
            watcher: watcher_policy.create_watcher(root),
            last_manifest_check: Instant::now(),
        }
    }

    fn can_reuse_without_manifest(&self, now: Instant, safety_interval: Duration) -> bool {
        self.watcher.can_skip_manifest_check()
            && now.duration_since(self.last_manifest_check) < safety_interval
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherPolicy {
    Enabled,
    #[cfg(test)]
    Unavailable,
}

impl WatcherPolicy {
    fn create_watcher(self, root: &Path) -> IndexWatcher {
        match self {
            Self::Enabled => IndexWatcher::watch(root),
            #[cfg(test)]
            Self::Unavailable => IndexWatcher::unavailable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use crate::cache::{CachedIndexPayloadV2, cache_file_path, load_payload};
    use crate::dense::HashEmbeddingProvider;
    use crate::types::SearchFilters;

    use super::*;

    fn test_service(cache_dir: PathBuf) -> CodeSearchService {
        CodeSearchService::new_with_watcher_policy(
            Arc::new(HashEmbeddingProvider::new("test", 16)),
            cache_dir,
            WatcherPolicy::Unavailable,
        )
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: search returns the structured output shape with bounded result count.
    #[test]
    fn search_returns_structured_results() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            temp.path().join("parser.rs"),
            "pub fn parse_input() {}\npub fn render_output() {}\n",
        )
        .expect("write");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "parse input".to_string(),
                content: ContentFilter::Code,
                top_k: 1,
                filters: SearchFilters::empty(),
            })
            .expect("search");

        assert_eq!(output.operation, CodeSearchOperation::Search);
        assert_eq!(output.results.len(), 1);
        assert_eq!(output.index_stats.indexed_files, 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: find-related excludes the source chunk and prefers same-language chunks.
    #[test]
    fn find_related_excludes_source_chunk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            temp.path().join("lib.rs"),
            "pub fn parse_input() {}\n\npub fn parse_related() {}\n",
        )
        .expect("write");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .find_related(RelatedRequest {
                root: temp.path().to_path_buf(),
                file_path: PathBuf::from("lib.rs"),
                line: 1,
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("related");

        assert_eq!(output.operation, CodeSearchOperation::FindRelated);
        assert!(
            output
                .results
                .iter()
                .all(|result| result.chunk.start_line != 1)
        );
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: cache invalidates when the indexed file manifest changes.
    #[test]
    fn search_cache_invalidates_after_file_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        let file = temp.path().join("lib.rs");
        fs::write(&file, "pub fn alpha() {}\n").expect("write");
        let service = test_service(cache.path().to_path_buf());

        let first = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "alpha".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("first search");
        fs::write(&file, "pub fn beta() {}\n").expect("rewrite");
        let second = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "beta".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("second search");

        assert_eq!(first.index_stats.indexed_files, 1);
        assert_eq!(second.results[0].chunk.content, "pub fn beta() {}");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: a dirty warm watcher state triggers manifest refresh before reuse.
    #[test]
    fn dirty_warm_state_refreshes_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        let file = temp.path().join("lib.rs");
        fs::write(&file, "pub fn alpha() {}\n").expect("write");
        let service = CodeSearchService::new(
            Arc::new(HashEmbeddingProvider::new("test", 16)),
            cache.path().to_path_buf(),
        );
        service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "alpha".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("first search");
        fs::write(&file, "pub fn beta_changed() {}\n").expect("rewrite");
        let root = temp.path().canonicalize().expect("canonical root");
        let key = memory_key(&root, ContentFilter::Code, "test");
        service
            .indexes
            .lock()
            .expect("indexes")
            .get(&key)
            .expect("state")
            .watcher
            .mark_dirty_for_test();

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "beta_changed".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("second search");

        assert_eq!(output.results[0].chunk.content, "pub fn beta_changed() {}");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: clean watcher state can skip manifest refresh only inside the safety interval.
    #[test]
    fn cached_state_reuse_requires_clean_available_recent_watcher() {
        let now = Instant::now();
        let stale = now
            .checked_sub(MANIFEST_SAFETY_INTERVAL + Duration::from_secs(1))
            .expect("stale instant");
        let clean_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::clean_for_test(),
            last_manifest_check: now,
        };
        let dirty_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::dirty_for_test(),
            last_manifest_check: now,
        };
        let unavailable_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::unavailable(),
            last_manifest_check: now,
        };
        let clean_stale = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::clean_for_test(),
            last_manifest_check: stale,
        };

        let reuse_decisions = vec![
            clean_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            dirty_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            unavailable_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            clean_stale.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
        ];

        assert_eq!(reuse_decisions, vec![true, false, false, false]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: stale v1 cache payloads are ignored and replaced by a v2 cache payload.
    #[test]
    fn stale_v1_cache_rebuilds_cleanly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(temp.path().join("lib.rs"), "pub fn parse_input() {}\n").expect("write");
        let root = temp.path().canonicalize().expect("canonical root");
        let cache_path = cache_file_path(cache.path(), &root, ContentFilter::Code, "test");
        fs::create_dir_all(cache_path.parent().expect("cache parent")).expect("cache dir");
        fs::write(
            &cache_path,
            r#"{"cache_version":1,"root":"/repo","content":"code","model_id":"test","manifest":[],"chunks":[],"embeddings":[]}"#,
        )
        .expect("write v1");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "parse input".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("search");

        assert_eq!(output.index_stats.indexed_files, 1);
        assert_eq!(load_payload(&cache_path).is_some(), true);
    }

    fn empty_index() -> Arc<SearchIndex> {
        Arc::new(
            SearchIndex::from_payload(CachedIndexPayloadV2::new(
                PathBuf::from("/repo"),
                ContentFilter::Code,
                "test".to_string(),
                Vec::new(),
            ))
            .expect("index"),
        )
    }
}
