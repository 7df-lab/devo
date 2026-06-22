//! Reasoning-effort picker data for the chat widget.
//!
//! Model capability resolution stays in `devo_protocol::Model`; this module
//! only converts those protocol choices into labels and list entries for the UI.

use devo_protocol::Model;
use devo_protocol::ReasoningEffort;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReasoningEffortListEntry {
    pub(crate) is_current: bool,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) value: String,
}

pub(super) fn reasoning_effort_entries_for_model(
    model: &Model,
    current_selection: &str,
) -> Vec<ReasoningEffortListEntry> {
    model
        .effective_reasoning_capability()
        .options()
        .into_iter()
        .map(|option| ReasoningEffortListEntry {
            is_current: option.value == current_selection
                || option.label.to_lowercase() == current_selection,
            label: option.label,
            description: option.description,
            value: option.value,
        })
        .collect()
}

pub(super) fn status_line_reasoning_effort_label(effort: Option<ReasoningEffort>) -> &'static str {
    match effort {
        Some(ReasoningEffort::None) | None => "default",
        Some(ReasoningEffort::Minimal) => "minimal",
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn status_line_reasoning_effort_labels_are_compact() {
        let actual = [
            status_line_reasoning_effort_label(None),
            status_line_reasoning_effort_label(Some(ReasoningEffort::Minimal)),
            status_line_reasoning_effort_label(Some(ReasoningEffort::XHigh)),
            status_line_reasoning_effort_label(Some(ReasoningEffort::Max)),
        ];

        assert_eq!(actual, ["default", "minimal", "xhigh", "max"]);
    }
}
