//! Sandbox profile picker data for the chat widget.
//!
//! The chat widget owns the selected sandbox profile name, while this module
//! keeps the label and picker-item mapping out of the main conversation
//! surface. Custom profiles are read from `sandbox.toml` via `devo_sandbox`.

use std::path::Path;

use devo_sandbox::ProfileName;
use devo_sandbox::load_sandbox_config;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::list_selection_view::SelectionItem;

/// `(value, label, description)` for each built-in profile, in picker order.
const BUILTIN_PROFILES: [(&str, &str, &str); 5] = [
    (
        "workspace",
        "Workspace",
        "Read anywhere; write only the workspace and essential directories.",
    ),
    (
        "devbox",
        "Devbox",
        "Read and write almost everything, except /data.",
    ),
    (
        "read-only",
        "Read-only",
        "Read anywhere; almost no writes; network blocked.",
    ),
    (
        "strict",
        "Strict",
        "Allowlisted system paths only; minimal writes; network blocked.",
    ),
    ("off", "Off", "No OS sandboxing for spawned commands."),
];

pub(super) fn sandbox_profile_items(current: &str, cwd: &Path) -> Vec<SelectionItem> {
    let mut items: Vec<SelectionItem> = BUILTIN_PROFILES
        .into_iter()
        .map(|(value, label, description)| {
            sandbox_profile_item(value, label, description.to_string(), value == current)
        })
        .collect();
    for name in custom_profile_names(cwd) {
        let is_current = name == current;
        items.push(sandbox_profile_item(
            &name,
            &name,
            "Custom profile from sandbox.toml".to_string(),
            is_current,
        ));
    }
    items
}

fn sandbox_profile_item(
    value: &str,
    label: &str,
    description: String,
    is_current: bool,
) -> SelectionItem {
    let profile = value.to_string();
    SelectionItem {
        name: label.to_string(),
        description: Some(description),
        is_current,
        dismiss_on_select: true,
        actions: vec![Box::new(move |app_event_tx| {
            app_event_tx.send(AppEvent::Command(AppCommand::UpdateSandboxProfile {
                profile: profile.clone(),
            }));
        })],
        ..Default::default()
    }
}

/// Display label for a profile name; custom profiles display as their name.
pub(super) fn sandbox_profile_label(profile: &str) -> &str {
    BUILTIN_PROFILES
        .iter()
        .find(|(value, ..)| *value == profile)
        .map(|(_, label, ..)| *label)
        .unwrap_or(profile)
}

/// Sorted custom profile names from `sandbox.toml`; empty when the config is
/// missing or cannot be loaded.
fn custom_profile_names(cwd: &Path) -> Vec<String> {
    let mut names: Vec<String> = load_sandbox_config(cwd)
        .map(|config| {
            config
                .profiles
                .into_keys()
                .filter(|name| matches!(name.parse(), Ok(ProfileName::Custom(_))))
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn sandbox_profile_labels_are_stable() {
        let actual: Vec<&str> = BUILTIN_PROFILES
            .iter()
            .map(|(value, ..)| sandbox_profile_label(value))
            .collect();

        assert_eq!(
            actual,
            ["Workspace", "Devbox", "Read-only", "Strict", "Off"]
        );
        assert_eq!(sandbox_profile_label("my-custom"), "my-custom");
    }

    #[test]
    fn sandbox_profile_items_mark_current_selection() {
        let items = sandbox_profile_items("read-only", Path::new("/nonexistent-workspace"));
        let builtin_rows: Vec<_> = items
            .iter()
            .take(BUILTIN_PROFILES.len())
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
            builtin_rows,
            vec![
                ("Workspace", true, false, true, 1),
                ("Devbox", true, false, true, 1),
                ("Read-only", true, true, true, 1),
                ("Strict", true, false, true, 1),
                ("Off", true, false, true, 1),
            ]
        );
        // The global sandbox.toml may add customs after the built-ins; none of
        // them may claim the current marker.
        assert_eq!(
            items.iter().filter(|item| item.is_current).count(),
            1,
            "exactly one item may be marked current"
        );
    }
}
