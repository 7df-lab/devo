import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./chat-turn.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const chatToolCallSource = readFileSync(new URL("./chat-tool-call.tsx", import.meta.url), "utf8")
const sharedReasoningSource = readFileSync(
	new URL("../../../../packages/ui/src/components/ai-elements/reasoning.tsx", import.meta.url),
	"utf8",
)
const responseActionsProps =
	source.match(/\{responseText && \([\s\S]*?<MessageActions([^>]*)>/)?.[1] ?? ""
const footerMetadataSource =
	source.match(/\{\/\* Per-turn metadata[\s\S]*?\{\/\* Turn-level message actions/)?.[0] ?? ""
const stepToggleDurationCount = source.match(/\{duration && `· \$\{duration\} `\}/g)?.length ?? 0
const stepsToggleIndex = source.indexOf("completedProcessExpanded && stepsToggle")
const defaultModeIndex = source.indexOf("Default mode: interleaved text + grouped tool summaries")
const verboseModeIndex = source.indexOf("Verbose mode: full tool cards")

describe("ChatTurnComponent transcript controls", () => {
	test("keeps completed process collapsed, suppresses zero-second footer, and shows actions", () => {
		expect({
			collapsesCompletedProcess: source.includes("const processSectionVisible =") &&
				source.includes("completedProcessExpanded"),
			suppressesSubSecondDuration: source.includes(
				'workTimeMs >= 1000 ? formatWorkDuration(workTimeMs) : ""',
			),
			usesActiveTurnDuration: source.includes("computeTurnWorkTime(turn, { active: working })"),
			stepsToggleBeforeDefaultStream:
				stepsToggleIndex !== -1 && defaultModeIndex !== -1 && stepsToggleIndex < defaultModeIndex,
			expandedStepsRenderBeforeFinalText:
				source.includes('const stepParts = processOrderedParts.filter((part) => part.kind !== "text")') &&
				source.includes('const textParts = processOrderedParts.filter((part) => part.kind === "text")') &&
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
			collapsesCompletedProcess: true,
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

	test("renders the active turn working timer between user message and assistant content", () => {
		const userMessageIndex = source.indexOf("{/* User message */}")
		const workingStripIndex = source.indexOf("<WorkingTurnStatusStrip")
		const assistantContentIndex = source.indexOf("{/* Tool calls + reasoning section */}")
		const responseTextIndex = source.indexOf("{/* Streaming response")

		expect({
			definesWorkingStrip: source.includes("function WorkingTurnStatusStrip"),
			usesWorkingForCopy: source.includes('Working for "'),
			reusesTurnDuration: source.includes("computeTurnWorkTime(turn, { active: true })"),
			placesStripAfterUserMessage:
				userMessageIndex !== -1 &&
				workingStripIndex !== -1 &&
				userMessageIndex < workingStripIndex,
			placesStripBeforeAssistantContent:
				workingStripIndex !== -1 &&
				assistantContentIndex !== -1 &&
				responseTextIndex !== -1 &&
				workingStripIndex < assistantContentIndex &&
				workingStripIndex < responseTextIndex,
			removesOldWorkingShimmer: !source.includes("Working shimmer"),
			keepsCompletedDurationAffordance: source.includes('Worked for "'),
		}).toEqual({
			definesWorkingStrip: true,
			usesWorkingForCopy: true,
			reusesTurnDuration: true,
			placesStripAfterUserMessage: true,
			placesStripBeforeAssistantContent: true,
			removesOldWorkingShimmer: true,
			keepsCompletedDurationAffordance: true,
		})
	})

	test("folds completed process details under Worked for while keeping final answer visible", () => {
		const disclosureIndex = source.indexOf("<CompletedTurnProcessDisclosure")
		const processSectionIndex = source.indexOf("{/* Tool calls + reasoning section */}")
		const completedFinalResponseIndex = source.indexOf("{/* Completed final response */}")

		expect({
			definesProcessDisclosure: source.includes("function CompletedTurnProcessDisclosure"),
			tracksCompletedProcessState: source.includes(
				"const [completedProcessExpanded, setCompletedProcessExpanded] = useState(false)",
			),
			splitsFinalResponseFromProcess: source.includes("splitCompletedTurnParts") &&
				source.includes("completedProcessParts") &&
				source.includes("finalResponsePart"),
			processSectionCanBeCollapsed: source.includes("const processSectionVisible =") &&
				source.includes("completedProcessExpanded"),
			stepsToggleHiddenWhenProcessCollapsed: source.includes(
				"completedProcessExpanded && stepsToggle",
			),
			disclosureReplacesCompletedDurationRow:
				disclosureIndex !== -1 && !source.includes("<CompletedTurnDurationRow"),
			finalResponseOutsideProcess:
				processSectionIndex !== -1 &&
				completedFinalResponseIndex !== -1 &&
				processSectionIndex < completedFinalResponseIndex,
			opensReasoningContentAfterExpandingWorkedFor: source.includes(
				"<TranscriptReasoningBlock",
			),
		}).toEqual({
			definesProcessDisclosure: true,
			tracksCompletedProcessState: true,
			splitsFinalResponseFromProcess: true,
			processSectionCanBeCollapsed: true,
			stepsToggleHiddenWhenProcessCollapsed: true,
			disclosureReplacesCompletedDurationRow: true,
			finalResponseOutsideProcess: true,
			opensReasoningContentAfterExpandingWorkedFor: true,
		})
	})

	test("keeps completed process disclosure reachable when duration is unavailable", () => {
		expect({
			disclosureAllowsMissingDuration: source.includes(
				"if (!duration && !hasProcessDetails) return null",
			),
			disclosureUsesWorkedFallback: source.includes('{duration ? "Worked for " : "Worked"}'),
			renderConditionIncludesProcessDetails: source.includes(
				"{!working && (duration || hasCompletedProcessDetails) && (",
			),
		}).toEqual({
			disclosureAllowsMissingDuration: true,
			disclosureUsesWorkedFallback: true,
			renderConditionIncludesProcessDetails: true,
		})
	})

	test("uses a subtle transcript-local reasoning indicator instead of the default Thought row", () => {
		const chatTurnComponentSource =
			source.match(/export const ChatTurnComponent = memo\([\s\S]*?\n\)/)?.[0] ?? ""

		expect({
			definesTranscriptReasoningBlock: source.includes("function TranscriptReasoningBlock"),
			definesTranscriptReasoningLiveCue: source.includes("function TranscriptReasoningLiveCue"),
			usesLeftRailReasoningStyle:
				source.includes("border-l") &&
				source.includes("aria-label=\"Reasoning details\""),
			removesBareReasoningTrigger: !chatTurnComponentSource.includes("<ReasoningTrigger />"),
			keepsSharedReasoningTriggerUnchanged:
				sharedReasoningSource.includes("export const ReasoningTrigger") &&
				sharedReasoningSource.includes("Thought for a few seconds"),
			dropsVisibleThoughtCopyDependency: !source.includes("Thought for a few seconds"),
			keepsActiveThinkingCue: source.includes("Thinking..."),
			completedReasoningRendersDirectly: source.includes(
				"<TranscriptReasoningBlock key={processItem.part.id} text={text} animated={isStreaming && i === item.items.length - 1} />",
			),
			verboseReasoningRendersDirectly: source.includes(
				"<TranscriptReasoningBlock text={reasoningText} animated={isReasoningStreaming} />",
			),
		}).toEqual({
			definesTranscriptReasoningBlock: true,
			definesTranscriptReasoningLiveCue: true,
			usesLeftRailReasoningStyle: true,
			removesBareReasoningTrigger: true,
			keepsSharedReasoningTriggerUnchanged: true,
			dropsVisibleThoughtCopyDependency: true,
			keepsActiveThinkingCue: true,
			completedReasoningRendersDirectly: true,
			verboseReasoningRendersDirectly: true,
		})
	})
})
