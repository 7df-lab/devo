//! Worker event dispatch for `ChatWidget`.
//!
//! This module keeps server/worker event handling out of the main chat surface
//! while preserving the existing state transitions and rendering side effects.

use std::time::Instant;

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;
use ratatui::text::Line;

use crate::app_event::AppEvent;
use crate::bottom_pane::ApprovalOverlay;
use crate::bottom_pane::ApprovalOverlayRequest;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::ExecCell;
use crate::exec_cell::new_active_exec_command;
use crate::get_git_diff::get_git_diff;
use crate::history_cell;
use crate::tool_result_cell::ToolResultCell;
use devo_util_shell_command::parse_command::parse_command;

use super::ActiveToolCall;
use super::ChatWidget;
use super::DotStatus;
use super::PendingApprovalRequest;
use super::SKILLS_TRANSCRIPT_TITLE;
use super::text_stream::ActiveTextItemId;

impl ChatWidget {
    fn start_command_execution_cell(
        &mut self,
        tool_use_id: String,
        title: String,
        command: Vec<String>,
        parsed: Vec<ParsedCommand>,
        source: ExecCommandSource,
    ) {
        if matches!(source, ExecCommandSource::UserShell) {
            self.current_turn_has_user_shell_command = true;
        }

        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && let Some(grouped) = cell.with_added_call(
                tool_use_id.clone(),
                command.clone(),
                parsed.clone(),
                source,
                None,
            )
        {
            *cell = grouped;
            self.active_tool_calls.insert(
                tool_use_id.clone(),
                ActiveToolCall {
                    tool_use_id,
                    title,
                    lines: Vec::new(),
                    exec_like: true,
                    start_time: None,
                },
            );
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.frame_requester.schedule_frame();
            self.set_status_message("Tool started");
            return;
        }

        self.flush_active_cell();
        self.active_cell = Some(Box::new(new_active_exec_command(
            tool_use_id.clone(),
            command,
            parsed,
            source,
            None,
            true,
        )));
        self.active_tool_calls.insert(
            tool_use_id.clone(),
            ActiveToolCall {
                tool_use_id,
                title,
                lines: Vec::new(),
                exec_like: true,
                start_time: None,
            },
        );
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        self.frame_requester.schedule_frame();
        self.set_status_message("Tool started");
    }

    pub(crate) fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::SessionActivated { .. } => {}
            WorkerEvent::TurnStarted {
                model,
                thinking,
                reasoning_effort,
                turn_id,
                ..
            } => {
                self.active_turn_id = Some(turn_id);
                self.committed_server_assistant_in_turn = false;
                self.current_turn_has_user_shell_command = false;
                self.sync_session_catalog_model(model);
                self.thinking_selection = thinking;
                self.session.reasoning_effort = reasoning_effort;
                self.refresh_header_box();
                self.busy = true;
                self.active_text_items.clear();
                self.active_proposed_plan = None;
                self.stream_chunking_policy.reset();
                self.bottom_pane.set_task_running(true);
            }
            WorkerEvent::TextItemStarted { item_id, kind } => {
                self.flush_active_cell();
                self.start_text_item(ActiveTextItemId::Server(item_id), kind);
                self.set_status_message(match kind {
                    TextItemKind::Assistant => "Generating",
                    TextItemKind::Reasoning => "Thinking",
                });
            }
            WorkerEvent::TextItemDelta {
                item_id,
                kind,
                delta,
            } => {
                self.push_text_item_delta(ActiveTextItemId::Server(item_id), kind, &delta);
                self.set_status_message(match kind {
                    TextItemKind::Assistant => "Generating",
                    TextItemKind::Reasoning => "Thinking",
                });
            }
            WorkerEvent::TextItemCompleted {
                item_id,
                kind,
                final_text,
            } => {
                self.complete_text_item(ActiveTextItemId::Server(item_id), kind, final_text);
                self.set_status_message(match kind {
                    TextItemKind::Assistant => "Generating",
                    TextItemKind::Reasoning => "Thinking",
                });
            }
            WorkerEvent::ProposedPlanStarted { item_id } => {
                self.start_proposed_plan(item_id);
            }
            WorkerEvent::ProposedPlanDelta { item_id, delta } => {
                self.push_proposed_plan_delta(item_id, delta);
            }
            WorkerEvent::ProposedPlanCompleted {
                item_id,
                final_text,
            } => {
                self.complete_proposed_plan(item_id, final_text);
            }
            WorkerEvent::TextDelta(text) => {
                if !self.has_server_active_item(TextItemKind::Assistant) {
                    self.flush_active_cell();
                    self.push_text_item_delta(
                        ActiveTextItemId::Legacy(TextItemKind::Assistant),
                        TextItemKind::Assistant,
                        &text,
                    );
                }
                self.set_status_message("Generating");
            }
            WorkerEvent::ReasoningDelta(text) => {
                if !self.has_server_active_item(TextItemKind::Reasoning) {
                    self.flush_active_cell();
                    self.push_text_item_delta(
                        ActiveTextItemId::Legacy(TextItemKind::Reasoning),
                        TextItemKind::Reasoning,
                        &text,
                    );
                }
                self.set_status_message("Thinking");
            }
            WorkerEvent::AssistantMessageCompleted(text) => {
                if !self.committed_server_assistant_in_turn
                    && !self.has_server_active_item(TextItemKind::Assistant)
                    && !self
                        .active_text_items
                        .iter()
                        .any(|item| item.kind == TextItemKind::Assistant)
                {
                    self.complete_text_item(
                        ActiveTextItemId::Legacy(TextItemKind::Assistant),
                        TextItemKind::Assistant,
                        text,
                    );
                }
                self.set_status_message("Generating");
            }
            WorkerEvent::ReasoningCompleted(text) => {
                if !self.has_server_active_item(TextItemKind::Reasoning) {
                    self.complete_text_item(
                        ActiveTextItemId::Legacy(TextItemKind::Reasoning),
                        TextItemKind::Reasoning,
                        text,
                    );
                }
                self.set_status_message("Thinking");
            }
            WorkerEvent::ToolCall {
                tool_use_id,
                summary,
                preparing,
                parsed_commands,
            } => {
                let command = crate::exec_command::split_command_string(&summary);
                let parsed = parsed_commands.unwrap_or_else(|| parse_command(&command));
                let exec_like = !parsed.is_empty()
                    && parsed.iter().all(|parsed| {
                        !matches!(
                            parsed,
                            devo_protocol::parse_command::ParsedCommand::Unknown { .. }
                        )
                    });
                if exec_like && !preparing {
                    self.start_command_execution_cell(
                        tool_use_id,
                        summary,
                        command,
                        parsed,
                        ExecCommandSource::Agent,
                    );
                    return;
                }

                let title = if preparing
                    && (summary.starts_with("write ")
                        || summary.starts_with("write:")
                        || summary == "apply_patch")
                {
                    if summary == "apply_patch" {
                        "Preparing apply_patch...".to_string()
                    } else {
                        "Preparing write...".to_string()
                    }
                } else {
                    summary
                };
                let tool_call = ActiveToolCall {
                    tool_use_id: tool_use_id.clone(),
                    title: title.clone(),
                    lines: Vec::new(),
                    exec_like: false,
                    start_time: None,
                };
                if preparing {
                    self.pending_tool_calls.push(ActiveToolCall {
                        lines: Vec::new(),
                        start_time: Some(Instant::now()),
                        ..tool_call
                    });
                } else {
                    self.active_tool_calls
                        .insert(tool_use_id.clone(), tool_call);
                    let pending_title = if title.starts_with("Running ") {
                        title
                    } else {
                        format!("Running {title}")
                    };
                    self.pending_tool_calls.push(ActiveToolCall {
                        tool_use_id,
                        title: pending_title,
                        lines: Vec::new(),
                        exec_like: false,
                        start_time: Some(Instant::now()),
                    });
                }
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.frame_requester.schedule_frame();
                self.set_status_message("Tool started");
            }
            WorkerEvent::CommandExecutionStarted {
                tool_use_id,
                command,
                source,
                command_actions,
            } => {
                let command_parts = crate::exec_command::split_command_string(&command);
                self.start_command_execution_cell(
                    tool_use_id,
                    command,
                    command_parts,
                    command_actions,
                    source,
                );
            }
            WorkerEvent::ToolCallUpdated {
                tool_use_id,
                summary,
                parsed_commands,
            } => {
                if let Some(tool_call) = self.active_tool_calls.get_mut(&tool_use_id) {
                    tool_call.title = summary.clone();
                    tool_call.exec_like = true;
                }
                let command = crate::exec_command::split_command_string(&summary);
                if let Some(cell) = self
                    .active_cell
                    .as_mut()
                    .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                    && cell.update_call(&tool_use_id, command.clone(), parsed_commands.clone())
                {
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                    self.frame_requester.schedule_frame();
                    self.set_status_message("Tool updated");
                    return;
                }
                if self.history.iter_mut().rev().any(|cell| {
                    cell.as_any_mut()
                        .downcast_mut::<ExecCell>()
                        .is_some_and(|cell| {
                            cell.update_call(&tool_use_id, command.clone(), parsed_commands.clone())
                        })
                }) {
                    self.frame_requester.schedule_frame();
                    self.set_status_message("Tool updated");
                }
            }
            WorkerEvent::ToolOutputDelta { tool_use_id, delta } => {
                if let Some(tool_call) = self.active_tool_calls.get_mut(&tool_use_id) {
                    if tool_call.exec_like {
                        if let Some(cell) = self
                            .active_cell
                            .as_mut()
                            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                            && cell.append_output(&tool_use_id, &delta)
                        {
                            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                            self.frame_requester.schedule_frame();
                        }
                        return;
                    }
                    let line = Line::from(delta).patch_style(Self::tool_text_style());
                    if let Some(pending) = self
                        .pending_tool_calls
                        .iter_mut()
                        .find(|pending| pending.tool_use_id == tool_use_id)
                    {
                        pending.lines.push(line);
                    } else {
                        tool_call.lines.push(line);
                    }
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                    self.frame_requester.schedule_frame();
                }
            }
            WorkerEvent::ToolResult {
                tool_use_id,
                title,
                preview,
                is_error,
                truncated,
            } => {
                // Remove from pending viewport entries — it will be committed to history below.
                if let Some(pos) = self
                    .pending_tool_calls
                    .iter()
                    .position(|tc| tc.tool_use_id == tool_use_id)
                {
                    self.pending_tool_calls.remove(pos);
                }
                let dot_status = if is_error {
                    DotStatus::Failed
                } else {
                    DotStatus::Completed
                };
                let resolved_title =
                    self.active_tool_calls
                        .remove(&tool_use_id)
                        .unwrap_or(ActiveToolCall {
                            tool_use_id: tool_use_id.clone(),
                            title,
                            lines: Vec::new(),
                            exec_like: false,
                            start_time: None,
                        });

                if resolved_title.exec_like {
                    let output = CommandOutput {
                        exit_code: if is_error { 1 } else { 0 },
                        aggregated_output: preview.clone(),
                        formatted_output: preview.clone(),
                    };
                    let duration = std::time::Duration::from_millis(0);
                    if let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                    {
                        let completed = cell.complete_call(&tool_use_id, output.clone(), duration);
                        if completed {
                            if cell.is_exploring_cell() {
                                self.active_cell_revision =
                                    self.active_cell_revision.wrapping_add(1);
                                self.frame_requester.schedule_frame();
                            } else if cell.should_flush() {
                                self.flush_active_cell();
                            } else {
                                self.active_cell_revision =
                                    self.active_cell_revision.wrapping_add(1);
                                self.frame_requester.schedule_frame();
                            }
                            self.set_status_message(if is_error {
                                "Tool returned an error"
                            } else {
                                "Tool completed"
                            });
                            return;
                        }
                    }
                    if let Some(cell) = self.history.iter_mut().rev().find_map(|cell| {
                        cell.as_any_mut()
                            .downcast_mut::<ExecCell>()
                            .and_then(|cell| {
                                cell.complete_call(&tool_use_id, output.clone(), duration)
                                    .then_some(cell)
                            })
                    }) {
                        let _ = cell;
                        self.frame_requester.schedule_frame();
                        self.set_status_message(if is_error {
                            "Tool returned an error"
                        } else {
                            "Tool completed"
                        });
                        return;
                    }
                }

                let resolved_title = resolved_title.title;

                let title_line =
                    (!resolved_title.is_empty()).then(|| Self::ran_tool_line(&resolved_title));
                if title_line.is_some() || !preview.is_empty() || truncated {
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                    self.add_to_history(ToolResultCell::new(
                        title_line,
                        preview,
                        self.dot_prefix(dot_status),
                        Line::from("  "),
                        Self::tool_text_style(),
                        truncated,
                    ));
                }
                self.set_status_message(if is_error {
                    "Tool returned an error"
                } else {
                    "Tool completed"
                });
                if Self::should_auto_show_git_diff(&resolved_title, is_error) {
                    let tx = self.app_event_tx.clone();
                    tokio::spawn(async move {
                        let text = Self::format_git_diff_result(get_git_diff().await);
                        tx.send(AppEvent::DiffResult(text));
                    });
                }
            }
            WorkerEvent::ShellCommandFinished { exit_code } => {
                let interrupted = exit_code.is_none();
                let accent_color = self.active_accent_color();
                let cell = if interrupted {
                    history_cell::TurnSummaryCell::new_interrupted(
                        "Shell".to_string(),
                        accent_color,
                    )
                } else {
                    history_cell::TurnSummaryCell::new("Shell".to_string(), None, accent_color)
                };
                self.add_to_history(cell);
                self.set_status_message("Shell command completed");
                self.current_turn_has_user_shell_command = false;
            }
            WorkerEvent::PlanUpdated { explanation, steps } => {
                self.on_plan_updated(explanation, steps);
                self.set_status_message("Plan updated");
            }
            WorkerEvent::PatchApplied { changes } => {
                self.pending_tool_calls.clear();
                self.add_to_history(history_cell::new_patch_event(changes, &self.session.cwd));
                self.set_status_message("Patch applied");
            }
            WorkerEvent::ApprovalRequest {
                session_id,
                turn_id,
                approval_id,
                action_summary,
                justification,
                resource,
                available_scopes,
                path,
                host,
                target,
            } => {
                self.commit_active_streams(DotStatus::Completed);
                self.pending_approval = Some(PendingApprovalRequest {
                    session_id,
                    turn_id,
                    approval_id: approval_id.clone(),
                    action_summary: action_summary.clone(),
                });
                self.bottom_pane
                    .open_popup_view(Box::new(ApprovalOverlay::new(
                        ApprovalOverlayRequest {
                            session_id,
                            turn_id,
                            approval_id,
                            action_summary,
                            justification,
                            resource,
                            available_scopes,
                            path,
                            host,
                            target,
                        },
                        self.app_event_tx.clone(),
                        self.active_accent_color(),
                    )));
                self.busy = true;
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Approval required");
            }
            WorkerEvent::RequestUserInput {
                session_id,
                turn_id,
                request_id,
                questions,
            } => {
                self.commit_active_streams(DotStatus::Completed);
                self.bottom_pane
                    .open_request_user_input(session_id, turn_id, request_id, questions);
                self.busy = true;
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Input requested");
            }
            WorkerEvent::ApprovalDecision {
                approval_id: _,
                decision,
                scope,
            } => {
                self.pending_approval = None;
                let symbol = if decision == "approve" { "✔" } else { "✗" };
                self.add_to_history(history_cell::new_info_event(
                    format!("{symbol} Permission request {decision} ({scope})"),
                    None,
                ));
                self.bottom_pane.set_task_running(self.busy);
            }
            WorkerEvent::UsageUpdated {
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
            } => {
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = total_input_tokens;
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::TurnFinished {
                stop_reason,
                turn_count,
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
                prompt_token_estimate,
            } => {
                let was_interrupted = stop_reason.contains("Interrupted");
                self.commit_active_streams(DotStatus::Completed);
                if was_interrupted
                    && let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                {
                    cell.mark_failed();
                }
                self.flush_active_cell();
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                self.pending_approval = None;
                self.committed_server_assistant_in_turn = false;
                self.busy = false;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                let model_name = if self.current_turn_has_user_shell_command {
                    "Shell".to_string()
                } else {
                    self.session
                        .model
                        .as_ref()
                        .map(|m| m.display_name.clone())
                        .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                        .unwrap_or_default()
                };
                let accent_color = self.active_accent_color();
                let elapsed = self
                    .bottom_pane
                    .status_widget()
                    .map(|status| status.elapsed_seconds())
                    .filter(|&secs| secs > 0);
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Ready");
                let cell = if was_interrupted {
                    history_cell::TurnSummaryCell::new_interrupted(model_name, accent_color)
                } else {
                    history_cell::TurnSummaryCell::new(model_name, elapsed, accent_color)
                };
                self.add_to_history(cell);
                self.current_turn_has_user_shell_command = false;
            }
            WorkerEvent::TurnFailed {
                message,
                turn_count,
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                prompt_token_estimate,
                last_query_input_tokens,
            } => {
                self.resume_browser_loading = false;
                self.commit_active_streams(DotStatus::Failed);
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                self.pending_approval = None;
                self.committed_server_assistant_in_turn = false;
                self.busy = false;
                self.turn_count = turn_count;
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                let model_name = if self.current_turn_has_user_shell_command {
                    "Shell".to_string()
                } else {
                    self.session
                        .model
                        .as_ref()
                        .map(|m| m.display_name.clone())
                        .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                        .unwrap_or_default()
                };
                self.add_to_history(history_cell::TurnSummaryCell::new_interrupted(
                    model_name,
                    self.active_accent_color(),
                ));
                self.add_to_history(history_cell::new_error_event(message));
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Query failed; see error above");
                self.current_turn_has_user_shell_command = false;
            }
            WorkerEvent::ProviderValidationSucceeded { reply_preview } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_succeeded(reply_preview.clone());
                }
                self.drain_onboarding_transcript_events();
                self.add_to_history(history_cell::new_info_event(
                    format!("Validation reply: {reply_preview}"),
                    Some("provider validation succeeded".to_string()),
                ));
                self.busy = false;
                self.set_status_message("Saving provider");
            }
            WorkerEvent::ProviderValidationFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_failed(message.clone());
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("provider validation failed".to_string()),
                ));
                self.set_status_message("Provider validation failed");
            }
            WorkerEvent::ProviderVendorsListed { provider_vendors } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_vendors_listed(provider_vendors);
                }
                self.drain_onboarding_transcript_events();
            }
            WorkerEvent::ProviderVendorUpserted {
                provider_vendor,
                model_binding,
            } => {
                let onboarding_was_active = self.onboarding.is_some();
                if let Some(binding) = model_binding.as_ref() {
                    self.apply_session_model_binding(binding);
                }
                if self.onboarding.is_some() {
                    if let Some(onboarding) = self.onboarding.as_mut() {
                        onboarding.on_provider_saved(model_binding.as_ref());
                    }
                    self.drain_onboarding_transcript_events();
                    if let Some(result) = self
                        .onboarding
                        .as_mut()
                        .and_then(crate::onboarding_widget::OnboardingWidget::take_result)
                    {
                        self.handle_onboarding_result(result);
                    }
                }
                if !onboarding_was_active {
                    self.add_to_history(history_cell::new_info_event(
                        format!("Provider saved: {}", provider_vendor.name),
                        Some("provider upserted".to_string()),
                    ));
                }
            }
            WorkerEvent::ProviderVendorUpsertFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_provider_save_failed(message.clone());
                }
                self.drain_onboarding_transcript_events();
                self.busy = false;
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("provider upsert failed".to_string()),
                ));
                self.set_status_message("Provider save failed");
            }
            WorkerEvent::SessionsListed { sessions } => {
                self.resume_browser_loading = false;
                self.open_resume_browser(sessions);
            }
            WorkerEvent::SubagentDiscovered { agent } => {
                self.on_subagent_discovered(agent);
            }
            WorkerEvent::SubagentMonitor { event } => {
                self.on_subagent_monitor_event(event);
            }
            WorkerEvent::SkillsListed {
                body,
                skills,
                show_in_transcript,
            } => {
                self.bottom_pane.set_skill_mentions(Some(skills));
                if show_in_transcript {
                    self.add_padded_markdown_history(SKILLS_TRANSCRIPT_TITLE, &body);
                    self.set_status_message("Skills loaded");
                }
            }
            WorkerEvent::ReferenceSearchUpdated { snapshot } => {
                self.bottom_pane.on_reference_search_result(snapshot);
            }
            WorkerEvent::NewSessionPrepared {
                cwd,
                model,
                thinking,
                reasoning_effort,
                active_agent_label,
                last_query_total_tokens: _,
                last_query_input_tokens: _,
                total_cache_read_tokens: _,
            } => {
                self.resume_browser_loading = false;
                self.session.cwd = cwd;
                self.update_session_request_model(model);
                self.thinking_selection = thinking;
                self.session.reasoning_effort = reasoning_effort;
                self.session.active_agent_label = active_agent_label.clone();
                self.bottom_pane.set_active_agent_label(active_agent_label);
                self.reset_subagent_monitor();
                let should_append_header = self.history_has_non_header_content();
                self.active_cell = None;
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.active_tool_calls.clear();
                self.pending_tool_calls.clear();
                self.active_text_items.clear();
                self.committed_server_assistant_in_turn = false;
                self.stream_chunking_policy.reset();
                self.busy = false;
                self.turn_count = 0;
                self.total_input_tokens = 0;
                self.total_output_tokens = 0;
                self.total_cache_read_tokens = 0;
                self.last_query_total_tokens = 0;
                self.last_query_input_tokens = 0;
                self.prompt_token_estimate = 0;
                if should_append_header {
                    self.push_session_header(/*is_first_run*/ false, None);
                } else {
                    self.refresh_header_box();
                }
                self.set_status_message("New session ready; send a prompt to start it");
            }
            WorkerEvent::SessionSwitched {
                session_id,
                cwd,
                title,
                model,
                thinking,
                reasoning_effort,
                active_agent_label,
                total_input_tokens,
                total_output_tokens,
                total_cache_read_tokens,
                last_query_total_tokens,
                last_query_input_tokens,
                prompt_token_estimate,
                history_items,
                rich_history_items,
                loaded_item_count,
                pending_texts,
            } => {
                self.resume_browser_loading = false;
                self.session.cwd = cwd;
                if let Some(model) = model {
                    self.update_session_request_model(model);
                }
                self.thinking_selection = thinking;
                self.session.reasoning_effort = reasoning_effort;
                self.session.active_agent_label = active_agent_label.clone();
                self.bottom_pane.set_active_agent_label(active_agent_label);
                self.reset_subagent_monitor();
                self.history.clear();
                self.next_history_flush_index = 0;
                self.active_text_items.clear();
                self.committed_server_assistant_in_turn = false;
                self.stream_chunking_policy.reset();
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.total_cache_read_tokens = total_cache_read_tokens;
                self.last_query_total_tokens = last_query_total_tokens;
                self.last_query_input_tokens = last_query_input_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                if !self.rebuild_restored_session_history_from_rich_items(
                    &rich_history_items,
                    loaded_item_count,
                    &session_id,
                    title.as_deref(),
                ) {
                    self.rebuild_restored_session_history(
                        history_items,
                        loaded_item_count,
                        &session_id,
                        title.as_deref(),
                    );
                }
                // Restore pending queue state from the resumed session
                self.queued_count = pending_texts.len();
                self.bottom_pane.clear_pending_cells();
                for text in &pending_texts {
                    self.bottom_pane.push_pending_cell(text.clone());
                }
                self.busy = false;
                self.set_status_message("Session switched");
            }
            WorkerEvent::GoalStatusLoaded { goal } => {
                self.show_goal_status(goal);
            }
            WorkerEvent::GoalUpdated { goal } => {
                self.show_goal_updated(goal);
            }
            WorkerEvent::GoalReplaceConfirmationRequested {
                current_goal,
                objective,
            } => {
                self.show_goal_replace_confirmation(current_goal, objective);
            }
            WorkerEvent::GoalEditLoaded { goal } => {
                self.show_goal_edit_prompt(goal);
            }
            WorkerEvent::GoalCleared { cleared } => {
                self.show_goal_cleared(cleared);
            }
            WorkerEvent::GoalOperationFailed { message } => {
                self.show_goal_operation_failed(message);
            }
            WorkerEvent::SessionRenamed { session_id, title } => {
                self.add_to_history(history_cell::new_info_event(
                    format!("renamed {session_id} to {title}"),
                    None,
                ));
                self.set_status_message("Session renamed");
            }
            WorkerEvent::SessionCompactionStarted => {
                self.busy = true;
                self.bottom_pane.set_task_running(true);
                self.set_status_message("Session compaction in progress");
            }
            WorkerEvent::SessionCompacted {
                total_input_tokens,
                total_output_tokens,
                prompt_token_estimate,
            } => {
                self.busy = false;
                self.bottom_pane.set_task_running(false);
                self.total_input_tokens = total_input_tokens;
                self.total_output_tokens = total_output_tokens;
                self.prompt_token_estimate = prompt_token_estimate;
                self.add_to_history(history_cell::new_info_event(
                    "Session compaction done".to_string(),
                    None,
                ));
                self.set_status_message("Session compacted");
            }
            WorkerEvent::SessionCompactionFailed { message } => {
                self.busy = false;
                self.bottom_pane.set_task_running(false);
                self.add_to_history(history_cell::new_error_event_with_hint(
                    message,
                    Some("session compaction failed".to_string()),
                ));
                self.set_status_message("Session compaction failed");
            }
            WorkerEvent::SessionTitleUpdated {
                session_id: _,
                title,
            } => {
                self.set_status_message(format!("Session: {title}"));
            }
            WorkerEvent::InputHistoryLoaded { direction: _, text } => {
                self.bottom_pane.restore_input_from_history(text);
            }
            WorkerEvent::InputQueueUpdated { pending_count, .. } => {
                // If the queue shrunk, unqueue the oldest queued cells.
                while self.queued_count > pending_count {
                    self.unqueue_oldest_pending();
                }
                self.frame_requester.schedule_frame();
            }
            WorkerEvent::SteerAccepted { .. } => {
                self.set_status_message("Steer accepted");
            }
        }
    }
}
