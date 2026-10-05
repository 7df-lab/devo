//! Continual Harness \(H = (\rho, G, K, M)\) — types and shared-file IO.
//!
//! See `L2-DES-HARNESS-001`. Full refine planning lives in the server host.

mod apply;
mod auto;
mod digest;
mod inject;
mod refinements;
mod state;

pub use apply::{ApplyError, RefineEdit, RefineEditOp, RefineProposal, apply_proposal_re_read};
pub use auto::{
    AutoRefineSettings, DEFAULT_COMPACT_COOLDOWN_MS, DEFAULT_TURN_INTERVAL, should_auto_refine,
};
pub use digest::{
    HARNESS_DIGEST_HEADING, HARNESS_DIGEST_PREFIX, HARNESS_DIGEST_SUFFIX,
    filter_texts_for_summarizer, format_harness_state_for_prompt, is_harness_digest_only,
    reattach_harness_digest, strip_harness_digests, text_contains_harness_digest,
    wrap_harness_digest,
};
pub use inject::HarnessDigestInjector;
pub use refinements::{
    RefinementLogError, RefinementResultRecord, append_refinement, kernel_snapshot_manifest_path,
    kernel_snapshot_path, record_from_proposal, refinements_log_path,
};
pub use state::{
    HarnessEntry, HarnessKind, HarnessScope, HarnessState, HarnessStateError, RefinementEvent,
    SCHEMA_VERSION,
};
