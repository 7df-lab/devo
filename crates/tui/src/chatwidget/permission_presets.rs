//! Permission preset picker data for the chat widget.
//!
//! The chat widget owns the selected permission preset, while this module keeps
//! the label and picker-item mapping out of the main conversation surface.

use devo_protocol::PermissionPreset;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::list_selection_view::SelectionItem;

pub(super) fn permission_preset_items(current: PermissionPreset) -> Vec<SelectionItem> {
    [
        (
            PermissionPreset::ReadOnly,
            "Read Only",
            "Devo can read files in the current workspace. Approval is required to edit files, run commands, or access the internet.",
        ),
        (
            PermissionPreset::Default,
            "Default",
            "Devo can read and edit files in the current workspace, and run commands. Approval is required to access the internet or edit other files.",
        ),
        (
            PermissionPreset::AutoReview,
            "Auto-review",
            "Same workspace-write permissions as Default, but eligible approvals are routed through the auto-reviewer before interrupting you.",
        ),
        (
            PermissionPreset::FullAccess,
            "Full Access",
            "Devo can edit files outside this workspace and access the internet without asking for approval. Exercise caution when using.",
        ),
    ]
    .into_iter()
    .map(|(preset, label, description)| SelectionItem {
        name: label.to_string(),
        description: Some(description.to_string()),
        is_current: preset == current,
        dismiss_on_select: true,
        actions: vec![Box::new(move |app_event_tx| {
            app_event_tx.send(AppEvent::Command(AppCommand::UpdatePermissions {
                preset,
            }));
        })],
        ..Default::default()
    })
    .collect()
}

pub(super) fn permission_preset_label(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::ReadOnly => "Read Only",
        PermissionPreset::Default => "Default",
        PermissionPreset::AutoReview => "Auto-review",
        PermissionPreset::FullAccess => "Full Access",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn permission_preset_labels_are_stable() {
        let actual = [
            permission_preset_label(PermissionPreset::ReadOnly),
            permission_preset_label(PermissionPreset::Default),
            permission_preset_label(PermissionPreset::AutoReview),
            permission_preset_label(PermissionPreset::FullAccess),
        ];

        assert_eq!(
            actual,
            ["Read Only", "Default", "Auto-review", "Full Access"]
        );
    }

    #[test]
    fn permission_preset_items_mark_current_selection() {
        let items = permission_preset_items(PermissionPreset::AutoReview);
        let actual: Vec<_> = items
            .iter()
            .map(|item| {
                (
                    item.name.as_str(),
                    item.description.is_some(),
                    item.is_current,
                    item.dismiss_on_select,
                    item.actions.len(),
                )
            })
            .collect();

        assert_eq!(
            actual,
            vec![
                ("Read Only", true, false, true, 1),
                ("Default", true, false, true, 1),
                ("Auto-review", true, true, true, 1),
                ("Full Access", true, false, true, 1),
            ]
        );
    }
}
