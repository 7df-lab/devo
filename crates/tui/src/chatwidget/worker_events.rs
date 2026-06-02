//! Worker event dispatch for `ChatWidget`.
//!
//! This module keeps server/worker event handling out of the main chat surface
//! while preserving the existing state transitions and rendering side effects.

use std::time::Instant;

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
use devo_utils::shell_command::parse_command::parse_command;

use super::ActiveToolCall;
use super::ChatWidget;
use super::DotStatus;
use super::PendingApprovalRequest;
use super::SKILLS_TRANSCRIPT_TITLE;
use super::text_stream::ActiveTextItemId;

impl ChatWidget {
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
                self.update_session_request_model(model);
                self.thinking_selection = thinking;
                self.session.reasoning_effort = reasoning_effort;
                self.refresh_header_box();
                self.busy = true;
                self.active_text_items.clear();
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
                    if let Some(cell) = self
                        .active_cell
                        .as_mut()
                        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                        && let Some(grouped) = cell.with_added_call(
                            tool_use_id.clone(),
                            command.clone(),
                            parsed.clone(),
                            devo_protocol::protocol::ExecCommandSource::Agent,
                            None,
                        )
                    {
                        *cell = grouped;
                        self.active_tool_calls.insert(
                            tool_use_id.clone(),
                            ActiveToolCall {
                                tool_use_id,
                                title: summary,
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
                        devo_protocol::protocol::ExecCommandSource::Agent,
                        None,
                        true,
                    )));
                    self.active_tool_calls.insert(
                        tool_use_id.clone(),
                        ActiveToolCall {
                            tool_use_id,
                            title: summary,
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
                    lines: vec![Self::running_tool_line(&title)],
                    exec_like: false,
                    start_time: None,
                };
                if preparing {
                    self.pending_tool_calls.push(ActiveToolCall {
                        start_time: Some(Instant::now()),
                        ..tool_call
                    });
                } else {
                    self.active_tool_calls.insert(
                        tool_use_id.clone(),
                        ActiveToolCall {
                            start_time: None,
                            ..tool_call.clone()
                        },
                    );
                    self.add_history_entry_without_redraw(Box::new(
                        history_cell::AgentMessageCell::new_with_prefix(
                            tool_call.lines,
                            self.dot_prefix(DotStatus::Pending),
                            "  ",
                            false,
                        ),
                    ));
                }
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.frame_requester.schedule_frame();
                self.set_status_message("Tool started");
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
                    let line = Line::from(delta.clone()).patch_style(Self::tool_text_style());
                    tool_call.lines.push(line);
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
                self.commit_active_streams(DotStatus::Completed);
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
                let model_name = self
                    .session
                    .model
                    .as_ref()
                    .map(|m| m.display_name.clone())
                    .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                    .unwrap_or_default();
                let accent_color = self.active_accent_color();
                let elapsed = self
                    .bottom_pane
                    .status_widget()
                    .map(|status| status.elapsed_seconds())
                    .filter(|&secs| secs > 0);
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Ready");
                let was_interrupted = stop_reason.contains("Interrupted");
                let cell = if was_interrupted {
                    history_cell::TurnSummaryCell::new_interrupted(model_name, accent_color)
                } else {
                    history_cell::TurnSummaryCell::new(model_name, elapsed, accent_color)
                };
                self.add_to_history(cell);
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
                let model_name = self
                    .session
                    .model
                    .as_ref()
                    .map(|m| m.display_name.clone())
                    .or_else(|| self.session.model.as_ref().map(|m| m.slug.clone()))
                    .unwrap_or_default();
                self.add_to_history(history_cell::TurnSummaryCell::new_interrupted(
                    model_name,
                    self.active_accent_color(),
                ));
                self.add_to_history(history_cell::new_error_event(message));
                self.bottom_pane.set_task_running(false);
                self.set_status_message("Query failed; see error above");
            }
            WorkerEvent::ProviderValidationSucceeded { reply_preview } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_succeeded(reply_preview.clone());
                    if let Some(result) = onboarding.take_result() {
                        self.handle_onboarding_result(result);
                    }
                }
                self.add_to_history(history_cell::new_info_event(
                    format!("Validation reply: {reply_preview}"),
                    Some("provider validation succeeded".to_string()),
                ));
                self.busy = false;
                self.set_status_message("Onboarding complete");
            }
            WorkerEvent::ProviderValidationFailed { message } => {
                if let Some(onboarding) = self.onboarding.as_mut() {
                    onboarding.on_validation_failed(message.clone());
                }
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
            }
            WorkerEvent::ProviderVendorUpserted { provider_vendor } => {
                self.add_to_history(history_cell::new_info_event(
                    format!("Provider saved: {}", provider_vendor.name),
                    Some("provider upserted".to_string()),
                ));
            }
            WorkerEvent::SessionsListed { sessions } => {
                self.resume_browser_loading = false;
                self.open_resume_browser(sessions);
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
            WorkerEvent::NewSessionPrepared {
                cwd,
                model,
                thinking,
                reasoning_effort,
                last_query_total_tokens: _,
                last_query_input_tokens: _,
                total_cache_read_tokens: _,
            } => {
                self.resume_browser_loading = false;
                self.session.cwd = cwd;
                self.update_session_request_model(model);
                self.thinking_selection = thinking;
                self.session.reasoning_effort = reasoning_effort;
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
