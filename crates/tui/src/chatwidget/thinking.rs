//! Thinking and reasoning picker data for the chat widget.
//!
//! Model capability resolution stays in `devo_protocol::Model`; this module
//! only converts those protocol choices into labels and list entries for the UI.

use devo_protocol::Model;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningEffortPreset;
use devo_protocol::ThinkingCapability;
use devo_protocol::ThinkingImplementation;
use devo_protocol::ThinkingPreset;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThinkingListEntry {
    pub(crate) is_current: bool,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) value: String,
}

pub(super) fn thinking_entries_for_model(
    model: &Model,
    current_selection: &str,
) -> Vec<ThinkingListEntry> {
    model
        .effective_thinking_capability()
        .options()
        .into_iter()
        .map(|option| ThinkingListEntry {
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

pub(super) fn reasoning_effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "None",
        ReasoningEffort::Minimal => "Minimal",
        ReasoningEffort::Low => "Low",
        ReasoningEffort::Medium => "Medium",
        ReasoningEffort::High => "High",
        ReasoningEffort::XHigh => "Extra high",
        ReasoningEffort::Max => "max",
    }
}

pub(super) fn thinking_label(
    capability: &ThinkingCapability,
    implementation: Option<&ThinkingImplementation>,
    default_reasoning_effort: Option<ReasoningEffort>,
) -> Option<&'static str> {
    if matches!(capability, ThinkingCapability::Unsupported)
        || matches!(implementation, Some(ThinkingImplementation::Disabled))
    {
        return None;
    }

    match capability {
        ThinkingCapability::Unsupported => None,
        ThinkingCapability::Toggle => Some("thinking"),
        ThinkingCapability::ToggleWithLevels(levels) => default_reasoning_effort
            .or_else(|| levels.first().copied())
            .map(|effort| status_line_reasoning_effort_label(Some(effort))),
        ThinkingCapability::Levels(levels) => default_reasoning_effort
            .or_else(|| levels.first().copied())
            .map(|effort| status_line_reasoning_effort_label(Some(effort))),
    }
}

pub(super) fn reasoning_effort_options(model: &Model) -> Vec<ReasoningEffortPreset> {
    model.reasoning_effort_options()
}

pub(super) fn thinking_options(model: &Model) -> Vec<ThinkingPreset> {
    model.effective_thinking_capability().options()
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

    #[test]
    fn thinking_label_uses_default_or_first_level() {
        assert_eq!(
            thinking_label(
                &ThinkingCapability::Levels(vec![ReasoningEffort::Low, ReasoningEffort::High]),
                None,
                None,
            ),
            Some("low")
        );
        assert_eq!(
            thinking_label(
                &ThinkingCapability::Levels(vec![ReasoningEffort::Low, ReasoningEffort::High]),
                None,
                Some(ReasoningEffort::High),
            ),
            Some("high")
        );
    }
}
