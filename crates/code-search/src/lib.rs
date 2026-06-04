mod cache;
mod chunking;
mod dense;
mod files;
mod index;
mod ranking;
mod refresh;
mod service;
mod tokens;
mod types;
mod watch;

pub use dense::{EmbeddingProvider, HashEmbeddingProvider, Model2VecEmbeddingProvider};
pub use service::CodeSearchService;
pub use types::{
    Chunk, CodeSearchError, CodeSearchOperation, ContentFilter, ContentKind, DEFAULT_TOP_K,
    IndexStats, MAX_TOP_K, RelatedRequest, SearchFilters, SearchOutput, SearchRequest,
    SearchResult, validate_top_k,
};
