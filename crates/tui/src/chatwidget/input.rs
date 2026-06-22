//! Keyboard, paste, slash-command, and queued-message handling for `ChatWidget`.
//!
//! The host forwards TUI and app events to the chat widget; this module keeps
//! those input transitions separate from transcript rendering and configuration.

use std::path::Path;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use devo_protocol::InputItem;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::ansi_escape::ansi_escape_line;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::InputMode;
use crate::bottom_pane::InputResult;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use crate::onboarding_widget::OnboardingResult;
use crate::onboarding_widget::OnboardingTranscriptEvent;
use crate::slash_command::SlashCommand;
use devo_protocol::CollaborationMode;

use super::ChatWidget;
use super::ExternalEditorState;
use super::PickerMode;
use super::UserMessage;

impl ChatWidget {
    pub(crate) fn handle_key_event(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if self.is_subagent_selector_open() {
            self.handle_subagent_selector_key_event(key);
            return;
        }
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_subagent_selector();
            return;
        }
        if self.resume_browser_loading {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.resume_browser = None;
                    self.resume_browser_loading = false;
                    self.set_status_message("Ready");
                    self.frame_requester.schedule_frame();
                }
                _ => {}
            }
            return;
        }
        if self.resume_browser.is_some() {
            self.handle_resume_browser_key_event(key);
            return;
        }
        if self.onboarding.is_some() && Self::is_copy_shortcut(key) {
            return;
        }
        if self.onboarding.is_some() {
            if let Some(onboarding) = self.onboarding.as_mut() {
                onboarding.handle_key_event(key);
            }
            self.drain_onboarding_transcript_events();
            if let Some(result) = self
                .onboarding
                .as_mut()
                .and_then(crate::onboarding_widget::OnboardingWidget::take_result)
            {
                self.handle_onboarding_result(result);
            }
            self.frame_requester.schedule_frame();
            return;
        }
        if self.handle_selection_mode_key(key) {
            return;
        }
        match self.bottom_pane.handle_key_event(key) {
            InputResult::Submitted {
                text,
                text_elements,
                local_images,
                mention_bindings,
                input_mode,
                collaboration_mode,
            } => {
                let user_message = UserMessage {
                    text,
                    local_images,
                    remote_image_urls: Vec::new(),
                    text_elements,
                    mention_bindings,
                };
                if self.busy && !user_message.text.trim().is_empty() {
                    // Turn is active — show in bottom pane as pending cell.
                    self.bottom_pane
                        .push_pending_cell(user_message.text.clone());
                    self.queued_input_modes.push_back(input_mode);
                    self.queued_count += 1;
                    self.app_event_tx.send(AppEvent::Command(
                        AppCommand::user_turn_with_collaboration_mode(
                            input_items_for_user_message(&user_message),
                            Some(self.session.cwd.clone()),
                            self.user_turn_model(),
                            self.user_turn_model_binding_id(),
                            self.reasoning_effort_selection.clone(),
                            /*sandbox*/ None,
                            Some("on-request".to_string()),
                            collaboration_mode,
                        ),
                    ));
                    self.set_status_message("Message queued");
                } else {
                    self.submit_user_message_with_modes(
                        user_message,
                        collaboration_mode,
                        input_mode,
                    );
                }
            }
            InputResult::ShellCommand { command } => {
                if self.busy {
                    self.set_status_message("Cannot run shell command while generating");
                } else {
                    self.current_turn_mode = InputMode::Shell;
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::execute_shell_command(
                            command,
                        )));
                    self.set_status_message("Shell command submitted");
                }
            }
            InputResult::ShellInput { command } => {
                if self.busy {
                    self.set_status_message("Cannot run shell command while generating");
                } else {
                    self.current_turn_mode = InputMode::Shell;
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::submit_shell_input(command)));
                    self.set_status_message("Shell command submitted");
                }
            }
            InputResult::Command { command, argument } => {
                self.handle_slash_command(command, argument);
            }
            InputResult::ModelSelected { model } => match self.picker_mode.take() {
                Some(PickerMode::ReasoningEffort) => self.apply_reasoning_effort_selection(model),
                _ => self.handle_model_picker_selection(model),
            },
            InputResult::ThemeSelected { name } => {
                self.apply_theme_selection(name);
            }
            InputResult::None => {}
        }
    }

    pub(crate) fn handle_onboarding_key_event(&mut self, key: KeyEvent) -> bool {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return self.onboarding.is_some();
        }
        if self.onboarding.is_some() && Self::is_copy_shortcut(key) {
            return false;
        }
        let Some(onboarding) = self.onboarding.as_mut() else {
            return false;
        };
        onboarding.handle_key_event(key);
        self.drain_onboarding_transcript_events();
        if let Some(result) = self
            .onboarding
            .as_mut()
            .and_then(crate::onboarding_widget::OnboardingWidget::take_result)
        {
            self.handle_onboarding_result(result);
        }
        self.frame_requester.schedule_frame();
        true
    }

    pub(crate) fn is_copy_shortcut(key: KeyEvent) -> bool {
        matches!(key.code, KeyCode::Char('c' | 'C'))
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER))
    }

    pub(crate) fn handle_paste(&mut self, text: String) {
        if self.resume_browser.is_some() {
            return;
        }
        if let Some(onboarding) = self.onboarding.as_mut() {
            onboarding.handle_paste(text);
            self.drain_onboarding_transcript_events();
            self.frame_requester.schedule_frame();
            return;
        }
        self.bottom_pane.handle_paste(text);
    }

    pub(crate) fn pre_draw_tick(&mut self) {
        self.advance_startup_header_animation();
        self.run_stream_commit_tick();
        self.bottom_pane.pre_draw_tick();
    }

    pub(crate) fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Redraw => self.frame_requester.schedule_frame(),
            AppEvent::SubmitUserInput { text } => self.submit_text(text),
            AppEvent::PreparePlanSuggestionInput => {
                self.bottom_pane.set_input_mode(InputMode::Plan);
                self.set_status_message("Type plan feedback");
            }
            AppEvent::ModelSelected { model } => {
                self.handle_model_picker_selection(model);
            }
            AppEvent::ThemeSelected { name } => {
                self.apply_theme_selection(name);
            }
            AppEvent::ReasoningEffortSelected { value } => {
                self.set_reasoning_effort_selection(value)
            }
            AppEvent::StatusMessageChanged { message } => self.set_status_message(message),
            AppEvent::HistoryEntryRequested { .. } => {
                self.set_status_message("Persistent composer history is not available");
            }
            AppEvent::ClearTranscript => {
                self.clear_transcript_view();
            }
            AppEvent::Interrupt => self.set_status_message("Interrupted"),
            AppEvent::Command(command) => {
                if matches!(
                    &command,
                    AppCommand::RunUserShellCommand { command } if command == "session list"
                ) {
                    self.resume_browser = None;
                    self.resume_browser_loading = true;
                }
                if command == AppCommand::Compact {
                    self.busy = true;
                    self.bottom_pane.set_task_running(true);
                    self.set_status_message("Requesting session compaction");
                    return;
                }
                self.set_status_message(format!("Command queued: {}", command.kind()));
            }
            AppEvent::RunSlashCommand { command } => {
                if let Ok(command) = command.parse::<SlashCommand>() {
                    self.handle_slash_command(command, String::new());
                }
                self.frame_requester.schedule_frame();
            }
            AppEvent::Exit(_)
            | AppEvent::OpenSlashCommandPopup
            | AppEvent::ClosePopup
            | AppEvent::OpenModelPicker
            | AppEvent::OpenReasoningEffortPicker
            | AppEvent::OpenThemePicker
            | AppEvent::OpenSubagentOverlay { .. }
            | AppEvent::StatusLineBranchUpdated { .. }
            | AppEvent::ReferenceSearchRequested { .. }
            | AppEvent::ReferenceSearchCancelled
            | AppEvent::StatusLineSetup { .. }
            | AppEvent::StatusLineSetupCancelled
            | AppEvent::TerminalTitleSetup { .. }
            | AppEvent::TerminalTitleSetupPreview { .. }
            | AppEvent::TerminalTitleSetupCancelled => {
                self.frame_requester.schedule_frame();
            }
            AppEvent::ReferenceSearchResults { snapshot } => {
                self.bottom_pane.on_reference_search_result(snapshot);
                self.frame_requester.schedule_frame();
            }
            AppEvent::DiffResult(text) => {
                let lines: Vec<Line<'static>> = if text.trim().is_empty() {
                    vec!["No changes detected.".italic().into()]
                } else {
                    text.lines().map(ansi_escape_line).collect()
                };
                let mut all_lines = vec![Line::from("Git Diff".bold()), Line::from("")];
                all_lines.extend(lines);
                self.add_to_history(PlainHistoryCell::new(all_lines));
                self.set_status_message("Diff shown");
            }
        }
    }

    pub(crate) fn submit_text(&mut self, text: String) {
        self.submit_user_message(UserMessage::from(text));
    }

    pub(super) fn submit_user_message(&mut self, user_message: UserMessage) {
        self.submit_user_message_with_modes(
            user_message,
            CollaborationMode::Build,
            InputMode::Build,
        );
    }

    pub(super) fn submit_user_message_with_modes(
        &mut self,
        user_message: UserMessage,
        collaboration_mode: CollaborationMode,
        input_mode: InputMode,
    ) {
        if user_message.text.trim().is_empty() {
            return;
        }

        self.current_turn_mode = input_mode;
        let local_image_paths = user_message
            .local_images
            .iter()
            .map(|attachment| attachment.path.clone())
            .collect::<Vec<_>>();
        let input = input_items_for_user_message(&user_message);
        self.add_to_history(history_cell::new_user_prompt(
            user_message.text.clone(),
            user_message.text_elements.clone(),
            local_image_paths,
            user_message.remote_image_urls.clone(),
            self.active_accent_color(),
            input_mode,
        ));

        self.app_event_tx.send(AppEvent::Command(
            AppCommand::user_turn_with_collaboration_mode(
                input,
                Some(self.session.cwd.clone()),
                self.user_turn_model(),
                self.user_turn_model_binding_id(),
                self.reasoning_effort_selection.clone(),
                /*sandbox*/ None,
                Some("on-request".to_string()),
                collaboration_mode,
            ),
        ));
        self.set_status_message("Submitted locally");
    }

    pub(super) fn handle_onboarding_result(&mut self, result: OnboardingResult) {
        match result {
            OnboardingResult::ValidationSucceeded {
                model_slug,
                model_name,
                display_name,
            } => {
                self.apply_session_model_name(model_slug, model_name, display_name);
                self.add_to_history(history_cell::new_info_event(
                    "Provider configured successfully".to_string(),
                    Some("onboarding complete".to_string()),
                ));
                self.onboarding = None;
                self.push_session_header(/*is_first_run*/ false, None);
                self.bottom_pane
                    .set_composer_input_enabled(/*enabled*/ true, /*placeholder*/ None);
                self.set_default_placeholder();
                self.set_status_message("Onboarding complete");
            }
            OnboardingResult::ValidationBypassed {
                model_slug,
                model_name,
                display_name,
            } => {
                self.apply_session_model_name(model_slug, model_name, display_name);
                self.add_to_history(history_cell::new_info_event(
                    "Provider added without validation".to_string(),
                    Some("onboarding validation skipped".to_string()),
                ));
                self.onboarding = None;
                self.push_session_header(/*is_first_run*/ false, None);
                self.bottom_pane
                    .set_composer_input_enabled(/*enabled*/ true, /*placeholder*/ None);
                self.set_default_placeholder();
                self.set_status_message("Onboarding complete");
            }
            OnboardingResult::Cancelled => {
                self.onboarding = None;
                self.app_event_tx
                    .send(AppEvent::Exit(crate::app_event::ExitMode::ShutdownFirst));
            }
        }
    }

    pub(super) fn drain_onboarding_transcript_events(&mut self) {
        let events = match self.onboarding.as_mut() {
            Some(onboarding) => onboarding.take_transcript_events(),
            None => return,
        };
        for event in events {
            self.add_to_history(PlainHistoryCell::new(
                self.onboarding_transcript_lines(event),
            ));
        }
    }

    fn onboarding_transcript_lines(&self, event: OnboardingTranscriptEvent) -> Vec<Line<'static>> {
        let marker = Span::styled("▌", Style::default().fg(self.active_accent_color()));
        match event {
            OnboardingTranscriptEvent::ModelSelected {
                model_slug,
                display_name,
            } => {
                let suffix = if model_slug == display_name {
                    String::new()
                } else {
                    format!(" ({model_slug})")
                };
                vec![Line::from(vec![
                    marker,
                    " ".into(),
                    "Onboarding model selected".bold(),
                    format!(" {display_name}{suffix}").into(),
                ])]
            }
            OnboardingTranscriptEvent::ProviderSelected {
                provider_name,
                base_url,
                credential_summary,
            } => {
                let mut lines = vec![Line::from(vec![
                    marker,
                    " ".into(),
                    "Onboarding provider selected".bold(),
                    format!(" {provider_name}").into(),
                ])];
                if let Some(base_url) = base_url {
                    lines.push(Line::from(format!("  base URL: {base_url}").dim()));
                }
                lines.push(Line::from(
                    format!("  credentials: {credential_summary}").dim(),
                ));
                lines
            }
            OnboardingTranscriptEvent::SettingsConfirmed {
                provider_name,
                base_url,
                model_name,
                display_name,
                invocation_method,
                default_reasoning_effort,
                credential_summary,
            } => {
                let mut lines = vec![Line::from(vec![
                    marker,
                    " ".into(),
                    "Onboarding settings confirmed".bold(),
                ])];
                lines.push(Line::from(format!("  provider: {provider_name}").dim()));
                if let Some(base_url) = base_url {
                    lines.push(Line::from(format!("  base URL: {base_url}").dim()));
                }
                lines.push(Line::from(format!("  request model: {model_name}").dim()));
                lines.push(Line::from(format!("  display name: {display_name}").dim()));
                lines.push(Line::from(format!("  wire API: {invocation_method}").dim()));
                lines.push(Line::from(
                    format!(
                        "  reasoning: {}",
                        default_reasoning_effort.unwrap_or_else(|| "default".to_string())
                    )
                    .dim(),
                ));
                lines.push(Line::from(
                    format!("  credentials: {credential_summary}").dim(),
                ));
                lines
            }
        }
    }

    pub(crate) fn external_editor_state(&self) -> ExternalEditorState {
        self.external_editor_state
    }

    pub(crate) fn set_external_editor_state(&mut self, state: ExternalEditorState) {
        self.external_editor_state = state;
    }

    pub(crate) fn queue_user_message(&mut self, user_message: UserMessage) {
        self.queued_user_messages.push_back(user_message);
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn restore_user_message_to_composer(&mut self, user_message: UserMessage) {
        self.bottom_pane
            .set_remote_image_urls(user_message.remote_image_urls);
        let local_image_paths = user_message
            .local_images
            .into_iter()
            .map(|attachment| attachment.path)
            .collect::<Vec<_>>();
        self.bottom_pane.set_text_content(
            user_message.text,
            user_message.text_elements,
            local_image_paths,
        );
        self.set_status_message("Previous message loaded");
    }

    pub(crate) fn pop_next_queued_user_message(&mut self) -> Option<UserMessage> {
        self.queued_user_messages.pop_front()
    }

    pub(crate) fn set_status_message(&mut self, message: impl Into<String>) {
        self.status_message = message.into();
        self.sync_bottom_pane_summary();
        self.frame_requester.schedule_frame();
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn last_plan_progress_for_test(&self) -> Option<(usize, usize)> {
        self.last_plan_progress
    }

    #[cfg(test)]
    pub(crate) fn input_mode_for_test(&self) -> InputMode {
        self.bottom_pane.input_mode()
    }

    pub(crate) fn composer_is_empty(&self) -> bool {
        self.bottom_pane.current_text().trim().is_empty()
    }

    pub(crate) fn is_normal_backtrack_mode(&self) -> bool {
        self.onboarding.is_none() && self.bottom_pane.is_normal_backtrack_mode()
    }

    pub(crate) fn show_esc_backtrack_hint(&mut self) {
        self.bottom_pane.show_esc_backtrack_hint();
    }

    pub(crate) fn clear_esc_backtrack_hint(&mut self) {
        self.bottom_pane.clear_esc_backtrack_hint();
    }

    pub(crate) fn is_onboarding_active(&self) -> bool {
        self.onboarding.is_some()
    }
}

fn input_items_for_user_message(user_message: &UserMessage) -> Vec<InputItem> {
    let mut input = vec![InputItem::Text {
        text: user_message.text.clone(),
    }];

    for binding in &user_message.mention_bindings {
        if is_skill_binding_path(&binding.path) {
            let name = binding
                .mention
                .strip_prefix('$')
                .unwrap_or(binding.mention.as_str());
            input.push(InputItem::Skill {
                name: name.to_string(),
                path: Path::new(&binding.path).to_path_buf(),
            });
            continue;
        }
        if binding.path.starts_with("mcp://") {
            input.push(InputItem::Mention {
                path: binding.path.clone(),
                name: Some(binding.mention.clone()),
            });
        }
    }

    input.extend(
        user_message
            .local_images
            .iter()
            .map(|attachment| InputItem::LocalImage {
                path: attachment.path.clone(),
            }),
    );
    input
}

fn is_skill_binding_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
}
