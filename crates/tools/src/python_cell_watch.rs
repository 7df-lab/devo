//! Python cell wait-budget decision types (harness-owned fg timeout → policy).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default first foreground wait before consulting wait policy (3 minutes).
pub const PYTHON_CELL_FIRST_WAIT_MS_DEFAULT: u64 = 180_000;
/// Minimum LLM-chosen continuation wait.
pub const PYTHON_CELL_WAIT_SECONDS_MIN: u64 = 30;
/// Maximum LLM-chosen continuation wait (15 minutes).
pub const PYTHON_CELL_WAIT_SECONDS_MAX: u64 = 15 * 60;
/// Max `continue_fg` renewals after the harness first wait; then force background.
pub const PYTHON_CELL_MAX_CONTINUE_RENEWALS: u32 = 3;
/// Tail size for stdout/stderr in the decision prompt.
pub const PYTHON_CELL_OUTPUT_TAIL_CHARS: usize = 2_000;
/// Code preview size for the decision prompt.
pub const PYTHON_CELL_CODE_PREVIEW_CHARS: usize = 1_500;

/// Structured wait-policy action after a Python cell exceeds its wait budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonCellWatchActionKind {
    ContinueFg,
    Background,
    Cancel,
}

/// Parsed LLM / mock decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonCellWatchAction {
    ContinueFg { wait_seconds: u64 },
    Background,
    Cancel,
}

/// Inputs for a mid-tool wait-policy decision.
#[derive(Debug, Clone)]
pub struct PythonCellWatchInput {
    pub cell_id: String,
    pub code_preview: String,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub renewals_remaining: u32,
}

/// Decision returned by [`PythonCellWatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonCellWatchDecision {
    pub action: PythonCellWatchAction,
    pub rationale: Option<String>,
}

/// Fired when a backgrounded Python cell reaches `done`.
#[derive(Debug, Clone)]
pub struct PythonCellCompletionEvent {
    pub session_id: String,
    pub cell_id: String,
    pub status: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub error: Option<String>,
}

/// Host hook for parked-cell completion notices / follow-up turns.
pub trait PythonCellCompletionHook: Send + Sync {
    fn completed(&self, event: PythonCellCompletionEvent);
}

/// Mid-tool callback that chooses continue / background / cancel for a long cell.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait PythonCellWatch: Send + Sync {
    async fn decide(&self, input: PythonCellWatchInput) -> PythonCellWatchDecision;
}

#[derive(Debug, Deserialize)]
struct WatchDecisionJson {
    action: String,
    #[serde(default)]
    wait_seconds: Option<f64>,
    #[serde(default)]
    rationale: Option<String>,
}

/// Clamp LLM `wait_seconds` into `[PYTHON_CELL_WAIT_SECONDS_MIN, PYTHON_CELL_WAIT_SECONDS_MAX]`.
pub fn clamp_wait_seconds(raw: u64) -> u64 {
    raw.clamp(PYTHON_CELL_WAIT_SECONDS_MIN, PYTHON_CELL_WAIT_SECONDS_MAX)
}

/// Resolve effective first-wait ms: explicit setting, else default 180s.
pub fn effective_first_wait_ms(setting: Option<u64>) -> u64 {
    setting.unwrap_or(PYTHON_CELL_FIRST_WAIT_MS_DEFAULT)
}

/// Truncate from the end for prompt tails.
pub fn output_tail(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let start = text
        .char_indices()
        .rev()
        .nth(max_chars.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("…{}", &text[start..])
}

/// Parse structured wait-policy JSON. Invalid / unknown → [`PythonCellWatchAction::Background`].
pub fn parse_python_cell_watch_decision(raw: &str) -> PythonCellWatchDecision {
    let trimmed = raw.trim();
    let parsed = extract_json_object(trimmed)
        .and_then(|s| serde_json::from_str::<WatchDecisionJson>(&s).ok())
        .or_else(|| serde_json::from_str::<WatchDecisionJson>(trimmed).ok());
    let Some(parsed) = parsed else {
        return PythonCellWatchDecision {
            action: PythonCellWatchAction::Background,
            rationale: Some("unparseable wait policy; defaulting to background".into()),
        };
    };
    let action = match parsed.action.trim().to_ascii_lowercase().as_str() {
        "continue_fg" | "continue" => {
            let secs = parsed
                .wait_seconds
                .map(|v| v.max(0.0) as u64)
                .unwrap_or(PYTHON_CELL_WAIT_SECONDS_MIN);
            PythonCellWatchAction::ContinueFg {
                wait_seconds: clamp_wait_seconds(secs),
            }
        }
        "cancel" => PythonCellWatchAction::Cancel,
        "background" | "bg" => PythonCellWatchAction::Background,
        _ => PythonCellWatchAction::Background,
    };
    PythonCellWatchDecision {
        action,
        rationale: parsed.rationale,
    }
}

fn extract_json_object(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    Some(raw[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_continue_clamps_wait_seconds() {
        let d = parse_python_cell_watch_decision(
            r#"{"action":"continue_fg","wait_seconds":5,"rationale":"soon"}"#,
        );
        assert_eq!(
            d.action,
            PythonCellWatchAction::ContinueFg {
                wait_seconds: PYTHON_CELL_WAIT_SECONDS_MIN
            }
        );
        let d =
            parse_python_cell_watch_decision(r#"{"action":"continue_fg","wait_seconds":99999}"#);
        assert_eq!(
            d.action,
            PythonCellWatchAction::ContinueFg {
                wait_seconds: PYTHON_CELL_WAIT_SECONDS_MAX
            }
        );
    }

    #[test]
    fn parse_invalid_defaults_to_background() {
        let d = parse_python_cell_watch_decision("not json");
        assert_eq!(d.action, PythonCellWatchAction::Background);
    }

    #[test]
    fn parse_cancel_and_background() {
        assert_eq!(
            parse_python_cell_watch_decision(r#"{"action":"cancel"}"#).action,
            PythonCellWatchAction::Cancel
        );
        assert_eq!(
            parse_python_cell_watch_decision(r#"{"action":"background"}"#).action,
            PythonCellWatchAction::Background
        );
    }

    #[test]
    fn force_background_when_renewals_exhausted() {
        assert_eq!(PYTHON_CELL_MAX_CONTINUE_RENEWALS, 3);
    }
}
