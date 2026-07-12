//! Semantic code retrieval for Devo.
//!
//! This crate implements the built-in read-only `code_search` tool's retrieval
//! engine: workspace discovery, code chunking, dense embeddings, sparse BM25,
//! hybrid ranking, related-code lookup, and warm/disk cache refresh. The public
//! surface is intentionally small so Devo's tool runtime owns schema validation
//! and execution policy while this crate owns retrieval mechanics.

mod cache;
mod chunking;
mod dense;
mod files;
mod grammars;
mod index;
mod matrix;
mod ranking;
mod refresh;
mod semantic;
mod service;
mod singleflight;
mod tokens;
mod types;
mod watch;

pub use dense::EmbeddingProvider;
pub use dense::HashEmbeddingProvider;
pub use dense::Model2VecEmbeddingProvider;
pub use service::CodeSearchService;
pub use types::Chunk;
pub use types::CodeSearchError;
pub use types::CodeSearchOperation;
pub use types::ContentFilter;
pub use types::ContentKind;
pub use types::DEFAULT_TOP_K;
pub use types::IndexStats;
pub use types::MAX_TOP_K;
pub use types::RelatedRequest;
pub use types::SearchFilters;
pub use types::SearchOutput;
pub use types::SearchRequest;
pub use types::SearchResult;
pub use types::validate_top_k;
