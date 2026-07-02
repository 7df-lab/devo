use super::research::ResearchUsageLedgerRef;
use super::research::ResearchUsageTotals;
use super::research_capture::{
    ClarificationQueryCapture, FinalReportWrite, PendingResearchToolCall,
    ResearchArtifactQueryCapture, ResearchQueryCapture, ResearchStageCapture,
    SupervisorQueryCapture,
};
use super::research_parsing::{
    is_request_user_input_tool_name, is_spawn_agent_tool_name,
    request_user_input_exchanges_from_response, request_user_input_questions_from_input,
    spawn_agent_child_session_id, spawn_agent_child_target, tool_content_to_json,
};
use super::research_stages::ResearchStageKind;
use super::research_stages::StreamedResearchArtifact;
use super::*;

pub(super) struct ResearchQueryEventContext<'a> {
    pub(super) session_id: SessionId,
    pub(super) turn_id: TurnId,
    pub(super) usage_ledger: &'a ResearchUsageLedgerRef,
    pub(super) context_window: Option<u64>,
}

struct ResearchArtifactEventContext<'a> {
    query: ResearchQueryEventContext<'a>,
    artifact: &'a StreamedResearchArtifact,
}

impl ServerRuntime {
    pub(super) async fn handle_research_stage_query_event(
        &self,
        context: ResearchQueryEventContext<'_>,
        stage: ResearchStageKind,
        capture: &mut ResearchStageCapture<'_>,
        artifact: Option<&StreamedResearchArtifact>,
        event: QueryEvent,
    ) -> anyhow::Result<()> {
        match capture {
            ResearchStageCapture::Clarification(capture) => {
                let artifact = artifact
                    .ok_or_else(|| anyhow::anyhow!("research {stage:?} missing artifact"))?;
                let artifact_context = ResearchArtifactEventContext {
                    query: context,
                    artifact,
                };
                self.handle_clarification_query_event(artifact_context, capture, event)
                    .await;
            }
            ResearchStageCapture::Artifact(capture) => {
                let artifact = artifact
                    .ok_or_else(|| anyhow::anyhow!("research {stage:?} missing artifact"))?;
                let artifact_context = ResearchArtifactEventContext {
                    query: context,
                    artifact,
                };
                self.handle_research_artifact_query_event(artifact_context, stage, capture, event)
                    .await;
            }
            ResearchStageCapture::Supervisor(capture) => {
                let artifact = artifact
                    .ok_or_else(|| anyhow::anyhow!("research supervisor missing artifact"))?;
                let artifact_context = ResearchArtifactEventContext {
                    query: context,
                    artifact,
                };
                self.handle_supervisor_query_event(artifact_context, capture, event)
                    .await;
            }
            ResearchStageCapture::FinalReport(capture) => {
                self.handle_final_report_query_event(context, capture, event)
                    .await;
            }
        }
        Ok(())
    }

    async fn handle_research_artifact_query_event(
        &self,
        context: ResearchArtifactEventContext<'_>,
        stage: ResearchStageKind,
        capture: &mut ResearchArtifactQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.query.session_id;
        let turn_id = context.query.turn_id;
        let usage_ledger = context.query.usage_ledger;
        let context_window = context.query.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_research_artifact_delta(
                    session_id,
                    turn_id,
                    &mut capture.artifact,
                    context.artifact,
                    text,
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!(
                    "{}_{}",
                    stage.usage_prefix(),
                    capture.usage_invocation_index
                );
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!(
                    "{}_{}",
                    stage.usage_prefix(),
                    capture.usage_invocation_index
                );
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } => {
                capture.turn_completed = true;
            }
            QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. } => {}
        }
    }

    async fn handle_supervisor_query_event(
        &self,
        context: ResearchArtifactEventContext<'_>,
        capture: &mut SupervisorQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.query.session_id;
        let turn_id = context.query.turn_id;
        let usage_ledger = context.query.usage_ledger;
        let context_window = context.query.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_research_artifact_delta(
                    session_id,
                    turn_id,
                    &mut capture.artifact,
                    context.artifact,
                    text,
                )
                .await;
            }
            QueryEvent::ToolUseStart { id, name, input } => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::ToolCall,
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: id.clone(),
                            tool_name: name.clone(),
                            parameters: input.clone(),
                            command_actions: Vec::new(),
                        })
                        .expect("serialize supervisor tool call payload"),
                    )
                    .await;
                capture.pending_tools.insert(
                    id,
                    PendingResearchToolCall {
                        item_id,
                        item_seq,
                        tool_name: name,
                        input,
                    },
                );
            }
            QueryEvent::ToolExecutionStart { .. } => {}
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                display_content,
                is_error,
                summary,
            } => {
                let output = tool_content_to_json(content);
                if is_spawn_agent_tool_name(&tool_name) && !is_error {
                    let child_session_id =
                        if let Some(child_session_id) = spawn_agent_child_session_id(&output) {
                            Some(child_session_id)
                        } else if let Some(target) = spawn_agent_child_target(&output) {
                            self.resolve_child_agent(session_id, &target)
                                .await
                                .ok()
                                .map(|metadata| metadata.session_id)
                        } else {
                            None
                        };
                    if let Some(child_session_id) = child_session_id {
                        self.remember_research_child_agent(session_id, child_session_id)
                            .await;
                        capture.spawned_worker_count += 1;
                    }
                }
                if let Some(pending) = capture.pending_tools.remove(&tool_use_id) {
                    self.complete_item(
                        session_id,
                        turn_id,
                        pending.item_id,
                        pending.item_seq,
                        ItemKind::ToolCall,
                        TurnItem::ToolCall(ToolCallItem {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name.clone(),
                            input: pending.input.clone(),
                        }),
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name,
                            parameters: pending.input,
                            command_actions: Vec::new(),
                        })
                        .expect("serialize completed supervisor tool call"),
                    )
                    .await;
                }
                self.emit_turn_item(
                    session_id,
                    turn_id,
                    ItemKind::ToolResult,
                    TurnItem::ToolResult(ToolResultItem {
                        tool_call_id: tool_use_id.clone(),
                        tool_name: Some(tool_name.clone()),
                        output: output.clone(),
                        display_content: display_content.clone(),
                        is_error,
                    }),
                    serde_json::to_value(ToolResultPayload {
                        tool_call_id: tool_use_id,
                        tool_name: Some(tool_name),
                        input: (!input.is_null()).then_some(input),
                        content: output,
                        display_content,
                        is_error,
                        summary,
                    })
                    .expect("serialize supervisor tool result payload"),
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("supervisor_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("supervisor_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } | QueryEvent::ToolProgress { .. } => {}
        }
    }

    async fn handle_final_report_query_event(
        &self,
        context: ResearchQueryEventContext<'_>,
        capture: &mut ResearchQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.session_id;
        let turn_id = context.turn_id;
        let usage_ledger = context.usage_ledger;
        let context_window = context.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_agent_message_delta(session_id, turn_id, &mut capture.assistant, text)
                    .await;
            }
            QueryEvent::ToolUseStart { id, name, input } => {
                let (item_id, item_seq) = self
                    .start_item(
                        session_id,
                        turn_id,
                        ItemKind::ToolCall,
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: id.clone(),
                            tool_name: name.clone(),
                            parameters: input.clone(),
                            command_actions: Vec::new(),
                        })
                        .expect("serialize final report tool call payload"),
                    )
                    .await;
                capture.pending_tools.insert(
                    id,
                    PendingResearchToolCall {
                        item_id,
                        item_seq,
                        tool_name: name,
                        input,
                    },
                );
            }
            QueryEvent::ToolExecutionStart { .. } => {}
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                display_content,
                is_error,
                summary,
            } => {
                let output = tool_content_to_json(content);
                if is_write_tool_name(&tool_name)
                    && !is_error
                    && let Some(path) = extract_written_file_path(&input, &output)
                    && let Some(content) = input
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .filter(|content| !content.trim().is_empty())
                {
                    capture.final_report_write = Some(FinalReportWrite {
                        path,
                        content: content.to_string(),
                    });
                }
                if let Some(pending) = capture.pending_tools.remove(&tool_use_id) {
                    self.complete_item(
                        session_id,
                        turn_id,
                        pending.item_id,
                        pending.item_seq,
                        ItemKind::ToolCall,
                        TurnItem::ToolCall(ToolCallItem {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name.clone(),
                            input: pending.input.clone(),
                        }),
                        serde_json::to_value(ToolCallPayload {
                            tool_call_id: tool_use_id.clone(),
                            tool_name: pending.tool_name,
                            parameters: pending.input,
                            command_actions: Vec::new(),
                        })
                        .expect("serialize completed final report tool call"),
                    )
                    .await;
                }
                self.emit_turn_item(
                    session_id,
                    turn_id,
                    ItemKind::ToolResult,
                    TurnItem::ToolResult(ToolResultItem {
                        tool_call_id: tool_use_id.clone(),
                        tool_name: Some(tool_name.clone()),
                        output: output.clone(),
                        display_content: display_content.clone(),
                        is_error,
                    }),
                    serde_json::to_value(ToolResultPayload {
                        tool_call_id: tool_use_id,
                        tool_name: Some(tool_name),
                        input: (!input.is_null()).then_some(input),
                        content: output,
                        display_content,
                        is_error,
                        summary,
                    })
                    .expect("serialize final report tool result payload"),
                )
                .await;
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("final_report_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. } => {
                capture.turn_completed = true;
            }
            QueryEvent::ToolProgress { .. } => {}
        }
    }

    async fn handle_clarification_query_event(
        &self,
        context: ResearchArtifactEventContext<'_>,
        capture: &mut ClarificationQueryCapture,
        event: QueryEvent,
    ) {
        let session_id = context.query.session_id;
        let turn_id = context.query.turn_id;
        let usage_ledger = context.query.usage_ledger;
        let context_window = context.query.context_window;
        match event {
            QueryEvent::TextDelta(text) => {
                capture.text.push_str(&text);
                self.push_research_artifact_delta(
                    session_id,
                    turn_id,
                    &mut capture.artifact,
                    context.artifact,
                    text,
                )
                .await;
            }
            QueryEvent::ToolUseStart {
                id, name, input, ..
            } => {
                if is_request_user_input_tool_name(&name) {
                    let questions = request_user_input_questions_from_input(&input);
                    if !questions.is_empty() {
                        capture
                            .pending_request_user_input_questions
                            .insert(id, questions);
                    }
                }
            }
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                ..
            } => {
                if is_request_user_input_tool_name(&tool_name) {
                    let output = tool_content_to_json(content);
                    if let Ok(response) =
                        serde_json::from_value::<devo_protocol::RequestUserInputResponse>(output)
                    {
                        let questions = capture
                            .pending_request_user_input_questions
                            .remove(&tool_use_id)
                            .unwrap_or_default();
                        let exchanges =
                            request_user_input_exchanges_from_response(&questions, &response);
                        capture.clarifications.extend(
                            exchanges
                                .iter()
                                .filter(|exchange| !exchange.answer.trim().is_empty())
                                .cloned(),
                        );
                        capture.request_user_input_exchanges.extend(exchanges);
                    }
                }
            }
            QueryEvent::Usage { usage } => {
                let usage_key = format!("clarify_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
                capture.usage_invocation_index += 1;
            }
            QueryEvent::UsageDelta { usage } => {
                let usage_key = format!("clarify_call_{}", capture.usage_invocation_index);
                self.apply_research_usage(
                    session_id,
                    turn_id,
                    usage_ledger,
                    usage_key,
                    ResearchUsageTotals::from_usage(&usage),
                    context_window,
                )
                .await;
            }
            QueryEvent::ReasoningDelta(text) => {
                self.push_reasoning_delta(session_id, turn_id, &mut capture.reasoning, text)
                    .await;
            }
            QueryEvent::ReasoningCompleted => {
                self.complete_reasoning_item(session_id, turn_id, &mut capture.reasoning)
                    .await;
            }
            QueryEvent::TurnComplete { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. } => {}
        }
    }
}
