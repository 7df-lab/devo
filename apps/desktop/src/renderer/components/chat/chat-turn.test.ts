import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./chat-turn.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const chatToolCallSource = readFileSync(new URL("./chat-tool-call.tsx", import.meta.url), "utf8")
const responseActionsProps =
	source.match(/\{responseText && \([\s\S]*?<MessageActions([^>]*)>/)?.[1] ?? ""
const footerMetadataSource =
	source.match(/\{\/\* Per-turn metadata[\s\S]*?\{\/\* Turn-level message actions/)?.[0] ?? ""
const stepToggleDurationCount = source.match(/\{duration && `· \$\{duration\} `\}/g)?.length ?? 0
const stepsToggleIndex = source.indexOf("{stepsToggle}")
const defaultModeIndex = source.indexOf("Default mode: interleaved text + grouped tool summaries")
const verboseModeIndex = source.indexOf("Verbose mode: full tool cards")

describe("ChatTurnComponent transcript controls", () => {
	test("keeps completed steps collapsed, suppresses zero-second footer, and shows actions", () => {
		expect({
			filtersToolsWhenCollapsed: source.includes(
				'orderedParts.filter((part) => part.kind !== "tool")',
			),
			suppressesSubSecondDuration: source.includes(
				'workTimeMs >= 1000 ? formatWorkDuration(workTimeMs) : ""',
			),
			usesActiveTurnDuration: source.includes("computeTurnWorkTime(turn, { active: working })"),
			stepsToggleBeforeDefaultStream:
				stepsToggleIndex !== -1 && defaultModeIndex !== -1 && stepsToggleIndex < defaultModeIndex,
			expandedStepsRenderBeforeFinalText:
				source.includes('const stepParts = orderedParts.filter((part) => part.kind !== "text")') &&
				source.includes('const textParts = orderedParts.filter((part) => part.kind === "text")') &&
				source.includes("{verboseOrderedParts.map((item) => {") &&
				verboseModeIndex !== -1 &&
				source.indexOf("{verboseOrderedParts.map((item) => {") > verboseModeIndex,
			stepsShareOneToggle:
				source.match(/const stepsToggle =/g)?.length === 1 &&
				!source.includes("Toggle to verbose view") &&
				!source.includes("Collapse back to default view"),
			footerConditionOmitsDuration: footerMetadataSource.includes(
				"turn.assistantMessages.length > 0 && (turnModel || turnCostStr)",
			),
			footerDoesNotRenderDuration: !footerMetadataSource.includes(
				"{duration && <span>{duration}</span>}",
			),
			stepsKeepDuration: stepToggleDurationCount === 1,
			usesAlwaysVisibleActions: responseActionsProps.trim() === "",
			usesHoverHiddenActions:
				responseActionsProps.includes("opacity-0") ||
				responseActionsProps.includes("group-hover/turn:opacity-100"),
		}).toEqual({
			filtersToolsWhenCollapsed: true,
			suppressesSubSecondDuration: true,
			usesActiveTurnDuration: true,
			stepsToggleBeforeDefaultStream: true,
			expandedStepsRenderBeforeFinalText: true,
			stepsShareOneToggle: true,
			footerConditionOmitsDuration: true,
			footerDoesNotRenderDuration: true,
			stepsKeepDuration: true,
			usesAlwaysVisibleActions: true,
			usesHoverHiddenActions: false,
		})
	})

	test("wires pending permission requests into the active chat turn", () => {
		expect({
			chatTurnAcceptsPendingPermission: source.includes("pendingPermission?: PendingPermission"),
			chatTurnRendersPermissionItem:
				source.includes("<PermissionItem") && source.includes("pendingPermission.request"),
			chatViewPassesPermissionToLastTurn: chatViewSource.includes(
				"pendingPermission={index === turns.length - 1 ? effectivePermission : undefined}",
			),
			inputKeepsNoTurnPermissionFallback: chatViewSource.includes(
				"turns.length === 0 && effectivePermission",
			),
			permissionReplyClearsPendingCard:
				chatViewSource.includes("removePermissionAtom") &&
				chatViewSource.includes("removePermission({ sessionId: permissionSessionId, permissionId })"),
			noTurnPermissionFallbackUsesClearingHandlers:
				chatViewSource.includes("onApprove={handleApprovePermission}") &&
				chatViewSource.includes("onDeny={handleDenyPermission}"),
			chatToolCallKeepsUnusedPermissionProp: chatToolCallSource.includes("permission?:"),
		}).toEqual({
			chatTurnAcceptsPendingPermission: true,
			chatTurnRendersPermissionItem: true,
			chatViewPassesPermissionToLastTurn: true,
			inputKeepsNoTurnPermissionFallback: true,
			permissionReplyClearsPendingCard: true,
			noTurnPermissionFallbackUsesClearingHandlers: true,
			chatToolCallKeepsUnusedPermissionProp: false,
		})
	})
})
