//! Exit summary formerly provided by `crates/tui::AppExit`.

use devo_core::SessionId;

/// Summary returned when the interactive client exits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppExit {
    /// Active session identifier at exit, when one exists.
    pub session_id: Option<SessionId>,
    /// Whether provider onboarding completed successfully during this run.
    pub onboarding_completed: bool,
    /// Total turns completed in the session.
    pub turn_count: usize,
    /// Total input tokens accumulated in the session.
    pub total_input_tokens: usize,
    /// Total output tokens accumulated in the session.
    pub total_output_tokens: usize,
    /// Display total tokens accumulated in the session.
    pub total_tokens: usize,
    /// Total cached input tokens accumulated in the session.
    pub total_cache_read_tokens: usize,
}
