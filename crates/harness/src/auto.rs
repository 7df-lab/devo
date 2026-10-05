//! Root-session auto-refine interval settings (L2-DES-HARNESS-001 DD-3).

use serde::{Deserialize, Serialize};

/// Default successful user-visible assistant turns between auto-refine runs.
pub const DEFAULT_TURN_INTERVAL: u32 = 25;

/// Default cooldown between compact-triggered auto-refine reviews (ms).
pub const DEFAULT_COMPACT_COOLDOWN_MS: u64 = 20 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRefineSettings {
    /// Root sessions default on; children never auto-refine.
    pub enabled: bool,
    pub turn_interval: u32,
    pub compact_enabled: bool,
    pub compact_cooldown_ms: u64,
}

impl Default for AutoRefineSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            turn_interval: DEFAULT_TURN_INTERVAL,
            compact_enabled: true,
            compact_cooldown_ms: DEFAULT_COMPACT_COOLDOWN_MS,
        }
    }
}

/// Whether the root session should schedule auto-refine after a successful turn.
pub fn should_auto_refine(
    settings: &AutoRefineSettings,
    successful_user_visible_turns_since: u32,
    is_root: bool,
    is_goal_continuation: bool,
) -> bool {
    if !is_root || is_goal_continuation || !settings.enabled {
        return false;
    }
    let interval = settings.turn_interval.max(1);
    successful_user_visible_turns_since > 0
        && successful_user_visible_turns_since.is_multiple_of(interval)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: defaults are enabled with interval 25.
    #[test]
    fn defaults_match_spec() {
        assert_eq!(
            AutoRefineSettings::default(),
            AutoRefineSettings {
                enabled: true,
                turn_interval: 25,
                compact_enabled: true,
                compact_cooldown_ms: DEFAULT_COMPACT_COOLDOWN_MS,
            }
        );
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: goal-continuation turns never trigger auto-refine.
    #[test]
    fn goal_continuation_skipped() {
        assert!(!should_auto_refine(
            &AutoRefineSettings::default(),
            25,
            true,
            true
        ));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: children never auto-refine.
    #[test]
    fn children_skipped() {
        assert!(!should_auto_refine(
            &AutoRefineSettings::default(),
            25,
            false,
            false
        ));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: interval fires on multiples of N for root user-visible turns.
    #[test]
    fn interval_fires_on_multiple() {
        assert!(should_auto_refine(
            &AutoRefineSettings::default(),
            25,
            true,
            false
        ));
        assert!(!should_auto_refine(
            &AutoRefineSettings::default(),
            24,
            true,
            false
        ));
    }
}
