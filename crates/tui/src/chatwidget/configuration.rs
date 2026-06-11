//! Model, thinking, theme, and permission configuration flows for `ChatWidget`.
//!
//! Picker construction and selection application live here so configuration UI
//! changes stay separate from transcript and input handling.

use devo_protocol::Model;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::ReasoningEffortPreset;
use devo_protocol::ThinkingCapability;
use devo_protocol::ThinkingImplementation;
use devo_protocol::ThinkingPreset;
use ratatui::style::Color;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::ModelPickerEntry;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::events::SavedModelEntry;
use crate::history_cell;

use super::ChatWidget;
use super::PendingModelSelection;
use super::PickerMode;
use super::permission_preset_items;
use super::permission_preset_label;
use super::thinking::ThinkingListEntry;

impl ChatWidget {
    pub(crate) fn set_model(&mut self, model: Model) {
        self.thinking_selection = model.default_thinking_selection();
        self.session.reasoning_effort = model
            .resolve_thinking_selection(self.thinking_selection.as_deref())
            .effective_reasoning_effort;
        self.session.provider = Some(model.provider_wire_api());
        self.session.model = Some(model);
        self.session.request_model = None;
        self.session.model_binding_id = None;
        self.current_model_binding_id = None;
        self.set_default_placeholder();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn update_session_request_model(&mut self, slug: String) {
        self.session.request_model = None;
        if let Some(entry) = self.saved_model_entry_by_binding_id(&slug).cloned() {
            self.apply_saved_model_entry_to_session(&entry);
            return;
        }
        self.current_model_binding_id = None;
        self.session.model_binding_id = None;
        self.sync_session_catalog_model(slug);
    }

    pub(super) fn update_session_model_selection(
        &mut self,
        slug: String,
        model_binding_id: Option<String>,
    ) {
        if let Some(binding_id) = model_binding_id {
            self.session.request_model = None;
            if let Some(entry) = self.saved_model_entry_by_binding_id(&binding_id).cloned() {
                self.apply_saved_model_entry_to_session(&entry);
                return;
            }
            self.sync_session_catalog_model(slug);
            self.current_model_binding_id = Some(binding_id.clone());
            self.session.model_binding_id = Some(binding_id);
            return;
        }

        self.update_session_request_model(slug);
    }

    pub(super) fn sync_session_catalog_model(&mut self, slug: String) {
        if let Some(model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        {
            self.session.reasoning_effort = model
                .resolve_thinking_selection(self.thinking_selection.as_deref())
                .effective_reasoning_effort;
            self.session.provider = Some(model.provider_wire_api());
            self.session.model = Some(model);
            return;
        }

        if let Some(model) = self.session.model.as_mut() {
            let display_name = if model.slug == slug && !model.display_name.is_empty() {
                model.display_name.clone()
            } else {
                slug.clone()
            };
            model.slug = slug.clone();
            model.display_name = display_name;
            self.session.reasoning_effort = model
                .resolve_thinking_selection(self.thinking_selection.as_deref())
                .effective_reasoning_effort;
            return;
        }

        self.session.model = Some(Model {
            slug: slug.clone(),
            display_name: slug,
            provider: self
                .session
                .provider
                .unwrap_or(ProviderWireApi::OpenAIChatCompletions),
            ..Model::default()
        });
        self.session.reasoning_effort = self
            .session
            .model
            .as_ref()
            .map(|model| model.resolve_thinking_selection(self.thinking_selection.as_deref()))
            .and_then(|resolved| resolved.effective_reasoning_effort);
    }

    pub(super) fn apply_session_model_name(
        &mut self,
        model_slug: String,
        model_name: String,
        display_name: String,
    ) {
        self.sync_session_catalog_model(model_slug.clone());
        self.session.request_model = if model_name == model_slug {
            None
        } else {
            Some(model_name)
        };
        let display_name = display_name.trim();
        if !display_name.is_empty()
            && let Some(model) = self.session.model.as_mut()
        {
            model.display_name = display_name.to_string();
        }
        if self.onboarding.is_none() {
            self.refresh_header_box();
        }
        self.sync_bottom_pane_summary();
        self.frame_requester.schedule_frame();
    }

    pub(super) fn apply_session_model_binding(&mut self, binding: &ProviderModelBinding) {
        self.apply_session_model_name(
            binding.model_slug.clone(),
            binding.model_name.clone(),
            binding
                .display_name
                .clone()
                .unwrap_or_else(|| binding.model_name.clone()),
        );
    }

    pub(super) fn user_turn_model(&self) -> Option<String> {
        self.session
            .request_model
            .clone()
            .or_else(|| self.session.model.as_ref().map(|model| model.slug.clone()))
    }

    pub(super) fn user_turn_model_binding_id(&self) -> Option<String> {
        self.session.model_binding_id.clone()
    }

    pub(crate) fn set_thinking_selection(&mut self, selection: Option<String>) {
        self.thinking_selection = selection;
        self.session.reasoning_effort = self
            .session
            .model
            .as_ref()
            .map(|model| model.resolve_thinking_selection(self.thinking_selection.as_deref()))
            .and_then(|resolved| resolved.effective_reasoning_effort);
        self.refresh_header_box();
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn current_thinking_selection(&self) -> Option<&str> {
        self.thinking_selection.as_deref()
    }

    pub(crate) fn current_reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.session.reasoning_effort.or_else(|| {
            self.session
                .model
                .as_ref()
                .map(|model| model.resolve_thinking_selection(self.thinking_selection.as_deref()))
                .and_then(|resolved| resolved.effective_reasoning_effort)
        })
    }

    pub(super) fn normalized_thinking_selection_for_display(
        &self,
        model: &Model,
    ) -> Option<String> {
        let current = self
            .thinking_selection
            .as_deref()
            .map(str::trim)
            .filter(|selection| !selection.is_empty())
            .map(str::to_ascii_lowercase)
            .or_else(|| model.default_thinking_selection());

        match model.effective_thinking_capability() {
            ThinkingCapability::ToggleWithLevels(_) => {
                if matches!(current.as_deref(), Some("disabled")) {
                    Some(String::from("disabled"))
                } else {
                    model
                        .resolve_thinking_selection(current.as_deref())
                        .effective_reasoning_effort
                        .map(|effort| effort.label().to_lowercase())
                }
            }
            _ => current,
        }
    }

    pub(super) fn display_thinking_selection(&self) -> Option<String> {
        let model = self.session.model.as_ref()?;
        self.normalized_thinking_selection_for_display(model)
    }

    pub(crate) fn thinking_entries(&self) -> Vec<ThinkingListEntry> {
        let Some(model) = &self.session.model else {
            return Vec::new();
        };

        let current = self
            .normalized_thinking_selection_for_display(model)
            .unwrap_or_default();

        model
            .effective_thinking_capability()
            .options()
            .into_iter()
            .map(|option| ThinkingListEntry {
                is_current: option.value == current || option.label.to_lowercase() == current,
                label: option.label,
                description: option.description,
                value: option.value,
            })
            .collect()
    }

    pub(crate) fn status_line_reasoning_effort_label(
        effort: Option<ReasoningEffort>,
    ) -> &'static str {
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

    pub(crate) fn reasoning_effort_label(effort: ReasoningEffort) -> &'static str {
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

    pub(crate) fn thinking_label(
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
                .map(|effort| Self::status_line_reasoning_effort_label(Some(effort))),
            ThinkingCapability::Levels(levels) => default_reasoning_effort
                .or_else(|| levels.first().copied())
                .map(|effort| Self::status_line_reasoning_effort_label(Some(effort))),
        }
    }

    pub(crate) fn reasoning_effort_options(model: &Model) -> Vec<ReasoningEffortPreset> {
        model.reasoning_effort_options()
    }

    pub(crate) fn thinking_options(model: &Model) -> Vec<ThinkingPreset> {
        model.effective_thinking_capability().options()
    }

    fn saved_model_selection_value(entry: &SavedModelEntry) -> &str {
        entry.binding_id.as_deref().unwrap_or(entry.model.as_str())
    }

    fn saved_model_display_name(entry: &SavedModelEntry) -> Option<String> {
        entry
            .display_name
            .as_deref()
            .or(entry.request_model.as_deref())
            .map(str::trim)
            .filter(|display_name| !display_name.is_empty())
            .map(ToOwned::to_owned)
    }

    fn saved_model_display_label(&self, entry: &SavedModelEntry) -> String {
        Self::saved_model_display_name(entry)
            .or_else(|| {
                self.available_models
                    .iter()
                    .find(|model| model.slug == entry.model)
                    .map(|model| model.display_name.clone())
            })
            .unwrap_or_else(|| entry.model.clone())
    }

    fn saved_model_provider_name(entry: &SavedModelEntry) -> Option<String> {
        entry
            .provider_name
            .as_deref()
            .or(entry.provider_id.as_deref())
            .map(str::trim)
            .filter(|provider_name| !provider_name.is_empty())
            .map(ToOwned::to_owned)
    }

    fn saved_model_entry_by_binding_id(&self, binding_id: &str) -> Option<&SavedModelEntry> {
        self.saved_models
            .iter()
            .find(|entry| entry.binding_id.as_deref() == Some(binding_id))
    }

    fn saved_model_entry_for_selection(&self, selection: &str) -> Option<&SavedModelEntry> {
        self.saved_model_entry_by_binding_id(selection).or_else(|| {
            self.saved_models.iter().find(|entry| {
                entry.model == selection || entry.request_model.as_deref() == Some(selection)
            })
        })
    }

    fn saved_model_entry_is_current(&self, entry: &SavedModelEntry) -> bool {
        if entry.binding_id.is_some()
            && self.current_model_binding_id.as_deref() == entry.binding_id.as_deref()
        {
            return true;
        }
        if self.current_model_binding_id.is_some() {
            return false;
        }
        self.session
            .model
            .as_ref()
            .is_some_and(|model| model.slug == entry.model)
            && self.session.request_model == entry.request_model
    }

    fn model_for_saved_entry(&self, entry: &SavedModelEntry) -> Model {
        let mut model = self
            .available_models
            .iter()
            .find(|model| model.slug == entry.model)
            .cloned()
            .unwrap_or_else(|| Model {
                slug: entry.model.clone(),
                ..Model::default()
            });
        if let Some(display_name) = Self::saved_model_display_name(entry) {
            model.display_name = display_name;
        }
        model.provider = entry.wire_api;
        model
    }

    fn apply_saved_model_entry_to_session(&mut self, entry: &SavedModelEntry) -> Model {
        let model = self.model_for_saved_entry(entry);
        self.session.provider = Some(entry.wire_api);
        self.session.model = Some(model.clone());
        self.session.request_model = entry.request_model.clone();
        self.session.model_binding_id = entry.binding_id.clone();
        self.current_model_binding_id = entry.binding_id.clone();
        model
    }

    pub(super) fn open_model_picker(&mut self) {
        self.picker_mode = Some(PickerMode::Model);
        self.pending_model_selection = None;
        let entries = self
            .saved_models
            .iter()
            .map(|entry| ModelPickerEntry {
                selection_value: Self::saved_model_selection_value(entry).to_string(),
                display_name: self.saved_model_display_label(entry),
                description: None,
                right_hint: Self::saved_model_provider_name(entry),
                is_current: self.saved_model_entry_is_current(entry),
            })
            .collect();
        self.bottom_pane.open_model_picker(entries);
        self.set_status_message("Select a model");
    }

    pub(super) fn handle_model_picker_selection(&mut self, slug: String) {
        if let Some(entry) = self.saved_model_entry_for_selection(&slug).cloned() {
            let selected_model = self.apply_saved_model_entry_to_session(&entry);
            let thinking_selection = selected_model.default_thinking_selection();
            self.pending_model_selection = Some(PendingModelSelection {
                selection: Self::saved_model_selection_value(&entry).to_string(),
                display_name: self.saved_model_display_label(&entry),
                thinking_selection: thinking_selection.clone(),
            });
            self.thinking_selection = thinking_selection;
            self.refresh_header_box();

            if selected_model
                .effective_thinking_capability()
                .options()
                .is_empty()
            {
                self.finalize_pending_model_selection();
                return;
            }

            self.open_thinking_picker();
            return;
        }

        let Some(selected_model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        else {
            self.apply_model_selection(slug);
            return;
        };

        let thinking_selection = selected_model.default_thinking_selection();
        self.pending_model_selection = Some(PendingModelSelection {
            selection: selected_model.slug.clone(),
            display_name: selected_model.display_name.clone(),
            thinking_selection: thinking_selection.clone(),
        });
        self.current_model_binding_id = None;
        self.session.model_binding_id = None;
        self.session.provider = Some(selected_model.provider);
        self.session.model = Some(selected_model.clone());
        self.session.request_model = None;
        self.thinking_selection = thinking_selection;
        self.refresh_header_box();

        if selected_model
            .effective_thinking_capability()
            .options()
            .is_empty()
        {
            self.finalize_pending_model_selection();
            return;
        }

        self.open_thinking_picker();
    }

    pub(super) fn open_theme_picker(&mut self) {
        self.bottom_pane
            .open_theme_picker(&self.theme_set.themes, self.active_theme_name.clone());
        self.set_status_message("Select a theme");
    }

    pub(super) fn open_permissions_picker(&mut self) {
        let current = self.permission_preset;
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                SelectionViewParams {
                    title: Some("Update Model Permissions".to_string()),
                    footer_hint: Some(Line::from("Press enter to confirm or esc to go back")),
                    items: permission_preset_items(current),
                    ..SelectionViewParams::default()
                },
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.set_status_message("Select permissions");
    }

    pub(crate) fn note_permissions_updated(&mut self, preset: devo_protocol::PermissionPreset) {
        self.permission_preset = preset;
        let label = permission_preset_label(preset);
        self.add_to_history(history_cell::new_info_event(
            format!("Permissions updated to {label}"),
            None,
        ));
        self.set_status_message(format!("Permissions updated to {label}"));
    }

    pub(super) fn apply_theme_selection(&mut self, name: String) {
        if let Some(theme) = self.theme_set.find(&name).cloned() {
            self.active_theme_name = name.clone();
            self.bottom_pane.set_accent_color(theme.accent_color);
            let _ = crate::onboarding::save_theme_selection(&name);
            self.set_status_message(format!("Theme set to {name}"));
            self.frame_requester.schedule_frame();
        }
    }

    pub(super) fn active_accent_color(&self) -> Color {
        self.theme_set
            .find(&self.active_theme_name)
            .map(|t| t.accent_color)
            .unwrap_or(Color::Cyan)
    }

    pub(super) fn active_error_color(&self) -> Color {
        self.theme_set
            .find(&self.active_theme_name)
            .map(|t| t.error_color)
            .unwrap_or(Color::Rgb(0xF8, 0x51, 0x49))
    }

    pub(super) fn apply_model_selection(&mut self, slug: String) {
        if let Some(entry) = self.saved_model_entry_for_selection(&slug).cloned() {
            let selected_model = self.apply_saved_model_entry_to_session(&entry);
            self.thinking_selection = selected_model.default_thinking_selection();
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::override_turn_context(
                    /*cwd*/ None,
                    Some(Self::saved_model_selection_value(&entry).to_string()),
                    Some(self.thinking_selection.clone()),
                    /*sandbox*/ None,
                    /*approval_policy*/ None,
                )));
            self.set_status_message(format!(
                "Model set to {}",
                self.saved_model_display_label(&entry)
            ));
            return;
        }

        if let Some(selected_model) = self
            .available_models
            .iter()
            .find(|model| model.slug == slug)
            .cloned()
        {
            self.thinking_selection = selected_model.default_thinking_selection();
            self.session.provider = Some(selected_model.provider);
            self.session.model = Some(selected_model.clone());
            self.session.request_model = None;
            self.session.model_binding_id = None;
            self.current_model_binding_id = None;
            self.app_event_tx
                .send(AppEvent::Command(AppCommand::override_turn_context(
                    /*cwd*/ None,
                    Some(selected_model.slug.clone()),
                    Some(self.thinking_selection.clone()),
                    /*sandbox*/ None,
                    /*approval_policy*/ None,
                )));
            self.set_status_message(format!("Model set to {}", selected_model.slug));
            return;
        }

        self.update_session_request_model(slug.clone());
        self.thinking_selection = self
            .session
            .model
            .as_ref()
            .and_then(Model::default_thinking_selection);
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::override_turn_context(
                /*cwd*/ None,
                Some(slug.clone()),
                Some(self.thinking_selection.clone()),
                /*sandbox*/ None,
                /*approval_policy*/ None,
            )));
        self.set_status_message(format!("Model set to {slug}"));
    }

    pub(super) fn open_thinking_picker(&mut self) {
        self.picker_mode = Some(PickerMode::Thinking);
        let entries = self.thinking_entries();
        if entries.is_empty() {
            self.set_status_message("Thinking Unsupported");
            return;
        }
        let model_entries = entries
            .into_iter()
            .map(|entry| ModelPickerEntry {
                selection_value: entry.value,
                display_name: entry.label,
                description: Some(entry.description),
                right_hint: None,
                is_current: entry.is_current,
            })
            .collect();
        self.bottom_pane.open_model_picker(model_entries);
        self.set_status_message("Select a thinking mode");
    }

    pub(super) fn apply_thinking_selection(&mut self, value: String) {
        self.thinking_selection = Some(value.clone());
        if let Some(pending) = self.pending_model_selection.as_mut() {
            pending.thinking_selection = Some(value);
            self.finalize_pending_model_selection();
            return;
        }

        self.refresh_header_box();
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::override_turn_context(
                /*cwd*/ None,
                /*model*/ None,
                Some(Some(value.clone())),
                /*sandbox*/ None,
                /*approval_policy*/ None,
            )));
        self.set_status_message(format!("Thinking set to {value}"));
    }

    pub(super) fn finalize_pending_model_selection(&mut self) {
        let Some(pending) = self.pending_model_selection.take() else {
            return;
        };

        self.picker_mode = None;
        self.thinking_selection = pending.thinking_selection.clone();
        self.refresh_header_box();
        self.app_event_tx
            .send(AppEvent::Command(AppCommand::override_turn_context(
                /*cwd*/ None,
                Some(pending.selection.clone()),
                Some(self.thinking_selection.clone()),
                /*sandbox*/ None,
                /*approval_policy*/ None,
            )));
        self.set_status_message(format!("Model set to {}", pending.display_name));
    }
}
