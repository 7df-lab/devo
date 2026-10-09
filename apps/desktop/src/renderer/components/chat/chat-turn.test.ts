import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./chat-turn.tsx", import.meta.url), "utf8")
const activitySource = readFileSync(new URL("./turn-activity.tsx", import.meta.url), "utf8")
const activityModelSource = readFileSync(new URL("./turn-activity-model.ts", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const permissionOptionsSource = readFileSync(
	new URL("./chat-permission-options.ts", import.meta.url),
	"utf8",
)
const eventProcessorSource = readFileSync(
	new URL("../../atoms/actions/event-processor.ts", import.meta.url),
	"utf8",
)
const chipSource = readFileSync(new URL("./composer-mode-chip.tsx", import.meta.url), "utf8")
const userMessageBlockSource = readFileSync(
	new URL("./user-message-block.tsx", import.meta.url),
	"utf8",
)

describe("ChatTurnComponent Native transcript", () => {
	test("renders Native ItemEnvelope rows without Message/Part dual tooling", () => {
		expect({
			nativeItemRow: source.includes("function NativeItemRow"),
			noProcessTimeline: !source.includes("ProcessTimelineView"),
			noBuildProcessTimeline: !source.includes("buildProcessTimeline"),
			noChatToolCall: !source.includes("ChatToolCall"),
			noThoughtRow: !source.includes("ThoughtRow"),
			usesItemsFamilyPath:
				source.includes("nativeItemType") && source.includes("assistantOrReasoningText"),
			activityUsesSharedDisclosure:
				source.includes("<TurnActivity") && activitySource.includes("<TranscriptDisclosure"),
			answersRemainVisible:
				!source.includes("setExpanded") && !source.includes("BotIcon"),
			completionIsQuiet:
				!activitySource.includes('"Done"') && !source.includes("item" + "s · working"),
			failedToolCompletionVisible:
				activityModelSource.includes('item.state === "failed"') && activityModelSource.includes('"Failed"'),
		}).toEqual({
			nativeItemRow: true,
			noProcessTimeline: true,
			noBuildProcessTimeline: true,
			noChatToolCall: true,
			noThoughtRow: true,
			usesItemsFamilyPath: true,
			activityUsesSharedDisclosure: true,
			answersRemainVisible: true,
			completionIsQuiet: true,
			failedToolCompletionVisible: true,
		})
	})

	test("keeps turn spacing aligned with transcript density", () => {
		expect(chatViewSource).toContain('"animate-in fade-in space-y-6 duration-150"')
	})

	test("routes pending permission requests through the composer flow", () => {
		expect({
			chatViewUsesComposerPermissionFlow:
				chatViewSource.includes("<ChatPermissionFlow") &&
				chatViewSource.includes("effectivePermission ?"),
			permissionOptionsFollowTuiShape:
				permissionOptionsSource.includes("buildApprovalChoices") &&
				permissionOptionsSource.includes('label: "Deny"') &&
				permissionOptionsSource.includes("Does not surface turn"),
			chatViewPrioritizesQuestionOverPermission:
				chatViewSource.includes("effectiveQuestion ?") &&
				chatViewSource.indexOf("effectiveQuestion ?") <
					chatViewSource.indexOf("effectivePermission ?"),
			permissionReplyClearsPendingCard:
				eventProcessorSource.includes('case "permission.replied"') &&
				eventProcessorSource.includes("removePermissionAtom"),
			composerPermissionUsesClearingHandlers:
				chatViewSource.includes("onApprove={handleApprovePermission}") &&
				chatViewSource.includes("onDeny={handleDenyPermission}"),
		}).toEqual({
			chatViewUsesComposerPermissionFlow: true,
			permissionOptionsFollowTuiShape: true,
			chatViewPrioritizesQuestionOverPermission: true,
			permissionReplyClearsPendingCard: true,
			composerPermissionUsesClearingHandlers: true,
		})
	})

	test("copies user messages and edits the latest user message while working", () => {
		expect({
			chatTurnUsesUserMessageBlock: source.includes("<UserMessageBlock"),
			editOnLatestTurnNotGatedByIdle: source.includes("canEdit={Boolean(onEditUserMessage)}"),
			chatViewPassesEditWhileWorking:
				chatViewSource.includes("latestEditableUserTurnIndex") &&
				chatViewSource.includes("onEditUserMessage(turn.userMessage.info.id, text)"),
			copiesUserMessage: userMessageBlockSource.includes(
				'tooltip={copied ? "Copied" : "Copy message"}',
			),
			editsLatestUserMessage: userMessageBlockSource.includes('tooltip="Edit message"'),
			resendsEditedMessage: userMessageBlockSource.includes(
				'{saving ? "Sending..." : "Send"}',
			),
		}).toEqual({
			chatTurnUsesUserMessageBlock: true,
			editOnLatestTurnNotGatedByIdle: true,
			chatViewPassesEditWhileWorking: true,
			copiesUserMessage: true,
			editsLatestUserMessage: true,
			resendsEditedMessage: true,
		})
	})

	test("keeps plan and skills entry points in chat view", () => {
		expect({
			modeToggle: chipSource.includes("Shift + Tab to toggle"),
			skillsSlash: chatViewSource.includes('case "skills":'),
			collaborationModeImport: chatViewSource.includes("collaborationModeFamily"),
		}).toEqual({
			modeToggle: true,
			skillsSlash: true,
			collaborationModeImport: true,
		})
	})
})
