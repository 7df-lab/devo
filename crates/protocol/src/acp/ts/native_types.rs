use ts_rs::{Config, TS};

use crate::native::{pi, turn, usage};

pub(super) fn append_pi_types(cfg: &Config, output: &mut String) {
    push_namespace::<pi::AgentMessage>(cfg, output, "NativePi");
    push_namespace::<pi::UserMessage>(cfg, output, "NativePi");
    push_namespace::<pi::UserContent>(cfg, output, "NativePi");
    push_namespace::<pi::UserContentBlock>(cfg, output, "NativePi");
    push_namespace::<pi::AssistantMessage>(cfg, output, "NativePi");
    push_namespace::<pi::AssistantContentBlock>(cfg, output, "NativePi");
    push_namespace::<pi::TextContent>(cfg, output, "NativePi");
    push_namespace::<pi::ThinkingContent>(cfg, output, "NativePi");
    push_namespace::<pi::ImageContent>(cfg, output, "NativePi");
    push_namespace::<pi::ToolCallData>(cfg, output, "NativePi");
    push_namespace::<pi::ToolCall>(cfg, output, "NativePi");
    push_namespace::<pi::Usage>(cfg, output, "NativePi");
    push_namespace::<pi::UsageCost>(cfg, output, "NativePi");
    push_namespace::<pi::StopReason>(cfg, output, "NativePi");
    push_namespace::<pi::AssistantMessageDiagnostic>(cfg, output, "NativePi");
    push_namespace::<pi::DiagnosticErrorInfo>(cfg, output, "NativePi");
    push_namespace::<pi::DiagnosticErrorCode>(cfg, output, "NativePi");
    push_namespace::<pi::ToolResultMessage>(cfg, output, "NativePi");
    push_namespace::<pi::AgentEvent>(cfg, output, "NativePi");
    push_namespace::<pi::AssistantMessageEvent>(cfg, output, "NativePi");
    push_namespace::<pi::SuccessfulStopReason>(cfg, output, "NativePi");
    push_namespace::<pi::FailedStopReason>(cfg, output, "NativePi");

    output.push_str(
        "export type AgentMessage = NativePi.AgentMessage;\n\
export type UserMessage = NativePi.UserMessage;\n\
export type UserContent = NativePi.UserContent;\n\
export type UserContentBlock = NativePi.UserContentBlock;\n\
export type AssistantMessage = NativePi.AssistantMessage;\n\
export type AssistantContentBlock = NativePi.AssistantContentBlock;\n\
export type TextContent = NativePi.TextContent;\n\
export type ThinkingContent = NativePi.ThinkingContent;\n\
export type ImageContent = NativePi.ImageContent;\n\
export type ToolCallData = NativePi.ToolCallData;\n\
export type ToolCall = NativePi.ToolCall;\n\
export type Usage = NativePi.Usage;\n\
export type UsageCost = NativePi.UsageCost;\n\
export type NativePiStopReason = NativePi.StopReason;\n\
export type AssistantMessageDiagnostic = NativePi.AssistantMessageDiagnostic;\n\
export type DiagnosticErrorInfo = NativePi.DiagnosticErrorInfo;\n\
export type DiagnosticErrorCode = NativePi.DiagnosticErrorCode;\n\
export type ToolResultMessage = NativePi.ToolResultMessage;\n\
export type AgentEvent = NativePi.AgentEvent;\n\
export type AssistantMessageEvent = NativePi.AssistantMessageEvent;\n\
export type SuccessfulStopReason = NativePi.SuccessfulStopReason;\n\
export type FailedStopReason = NativePi.FailedStopReason;\n\n",
    );
}

pub(super) fn append_turn_types(cfg: &Config, output: &mut String) {
    push_namespace::<turn::Turn>(cfg, output, "NativeTurn");
    push_namespace::<turn::TurnKind>(cfg, output, "NativeTurn");
    push_namespace::<turn::TurnStatus>(cfg, output, "NativeTurn");
    push_namespace::<usage::TurnUsage>(cfg, output, "NativeTurn");
    output.push_str(
        "export type Turn = NativeTurn.Turn;\n\
export type NativeTurnKind = NativeTurn.TurnKind;\n\
export type NativeTurnStatus = NativeTurn.TurnStatus;\n\
export type NativeTurnUsage = NativeTurn.TurnUsage;\n\n",
    );
}

fn push_namespace<T: TS>(cfg: &Config, output: &mut String, namespace: &str) {
    output.push_str(&format!(
        "export namespace {namespace} {{\n  export {}\n}}\n\n",
        T::decl(cfg)
    ));
}
