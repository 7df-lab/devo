//! Slash-command dispatch for `ChatWidget`.
//!
//! The bottom pane parses slash commands, and this module translates them into
//! chat-widget state changes or app commands sent back to the host loop.

use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::get_git_diff::get_git_diff;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use crate::slash_command::SlashCommand;

use super::ChatWidget;

impl ChatWidget {
    pub(super) fn can_change_configuration(&self) -> bool {
        !self.busy
    }

    pub(super) fn add_busy_configuration_message(&mut self, command: SlashCommand) {
        let noun = match command {
            SlashCommand::Model => "model",
            SlashCommand::Theme => "theme",
            SlashCommand::Compact => "session",
            SlashCommand::New => "session",
            SlashCommand::Resume => "session",
            SlashCommand::Permissions => "permissions",
            SlashCommand::Diff => "diff",
            SlashCommand::Mcp
            | SlashCommand::Skills
            | SlashCommand::Goal
            | SlashCommand::Exit
            | SlashCommand::Status
            | SlashCommand::Clear
            | SlashCommand::Btw => {
                return;
            }
        };
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
            "Cannot change {noun} while generating"
        ))]));
        self.set_status_message(format!("Cannot change {noun} while generating"));
    }

    pub(super) fn handle_slash_command(&mut self, command: SlashCommand, argument: String) {
        if !self.can_change_configuration() && !command.available_during_task() {
            self.add_busy_configuration_message(command);
            return;
        }

        match command {
            SlashCommand::Exit => {
                tracing::info!("slash /exit dispatched from chat widget");
                self.app_event_tx
                    .send(AppEvent::Exit(crate::app_event::ExitMode::ShutdownFirst));
            }
            SlashCommand::Clear => {
                self.history.clear();
                self.next_history_flush_index = 0;
                self.active_text_items.clear();
                self.stream_chunking_policy.reset();
                self.set_status_message("Transcript cleared");
            }
            SlashCommand::Status => {
                let model = self
                    .session
                    .model
                    .as_ref()
                    .map(|m| m.slug.as_str())
                    .unwrap_or("unknown");
                let thinking = self.thinking_selection.as_deref().unwrap_or("default");
                let cwd = self.session.cwd.display().to_string();
                let turns = self.turn_count;
                let tokens_in = Self::format_token_count(self.total_input_tokens);
                let tokens_out = Self::format_token_count(self.total_output_tokens);
                let lines = history_cell::with_border(vec![
                    Line::from("Session Status".bold()),
                    Line::from(""),
                    Line::from(format!("  model:       {model}")),
                    Line::from(format!("  thinking:    {thinking}")),
                    Line::from(format!("  cwd:         {cwd}")),
                    Line::from(format!("  turns:       {turns}")),
                    Line::from(format!(
                        "  tokens:      \u{2191}{tokens_in} \u{2193}{tokens_out}",
                    )),
                ]);
                self.add_to_history(PlainHistoryCell::new(lines));
                self.set_status_message("Session status shown");
            }
            SlashCommand::Permissions => {
                self.open_permissions_picker();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Model => {
                if argument.is_empty() {
                    self.open_model_picker();
                } else {
                    self.apply_model_selection(argument);
                }
            }
            SlashCommand::Mcp => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "mcp list".to_string(),
                    }));
                self.set_status_message("Loading MCP servers");
            }
            SlashCommand::Skills => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "skills list".to_string(),
                    }));
                self.set_status_message("Loading skills");
            }
            SlashCommand::Compact => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::compact()));
            }
            SlashCommand::New => {
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "session new".to_string(),
                    }));
                self.set_status_message("New session requested");
            }
            SlashCommand::Resume => {
                self.resume_browser = None;
                self.resume_browser_loading = true;
                self.app_event_tx
                    .send(AppEvent::Command(AppCommand::RunUserShellCommand {
                        command: "session list".to_string(),
                    }));
                self.set_status_message("Loading sessions");
            }
            SlashCommand::Btw => {
                if let Some(turn_id) = self.active_turn_id {
                    self.add_to_history(history_cell::new_user_prompt(
                        format!("/btw {argument}"),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        self.active_accent_color(),
                    ));
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::SteerTurn {
                            input: vec![devo_protocol::InputItem::Text { text: argument }],
                            expected_turn_id: turn_id,
                        }));
                    self.set_status_message("Steer sent");
                } else {
                    self.set_status_message("No active turn to steer");
                }
            }
            SlashCommand::Goal => {
                self.set_status_message("Goal management");
                self.add_to_history(PlainHistoryCell::new(vec![Line::from(
                    "Use /goal to view or manage the active goal. See goal/create, goal/pause, goal/resume in the protocol.",
                )]));
            }
            SlashCommand::Diff => {
                self.set_status_message("Computing diff");
                let tx = self.app_event_tx.clone();
                tokio::spawn(async move {
                    let text = Self::format_git_diff_result(get_git_diff().await);
                    tx.send(AppEvent::DiffResult(text));
                });
            }
        }
    }
}
