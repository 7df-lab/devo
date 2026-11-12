//! Shared distinction between provisional zero counters and useful fallback.

pub(crate) fn has_nonzero_counters(usage: &devo_protocol::Usage) -> bool {
    usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.total_tokens.unwrap_or(0) > 0
        || usage.cache_read_input_tokens.unwrap_or(0) > 0
        || usage.cache_creation_input_tokens.unwrap_or(0) > 0
        || usage.reasoning_output_tokens.unwrap_or(0) > 0
}
