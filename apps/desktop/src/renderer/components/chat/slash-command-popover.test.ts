import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const popoverSource = readFileSync(new URL("./slash-command-popover.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const clientCommandsBlock = popoverSource.match(/const CLIENT_COMMANDS[\s\S]*?\n\]/)?.[0] ?? ""

describe("Desktop slash command composer", () => {
	test("limits the popover to first-party composer commands", () => {
		expect({
			showsCompact: clientCommandsBlock.includes('name: "compact"'),
			showsGoal: clientCommandsBlock.includes('name: "goal"'),
			showsPlan: clientCommandsBlock.includes('name: "plan"'),
			showsResearch: clientCommandsBlock.includes('name: "research"'),
			omitsUndo: !clientCommandsBlock.includes('name: "undo"'),
			omitsSkills: !clientCommandsBlock.includes('name: "skills"'),
			omitsServerCommands: !popoverSource.includes("useServerCommands"),
			omitsUserCommandDispatch: !chatViewSource.includes("session.command"),
		}).toEqual({
			showsCompact: true,
			showsGoal: true,
			showsPlan: true,
			showsResearch: true,
			omitsUndo: true,
			omitsSkills: true,
			omitsServerCommands: true,
			omitsUserCommandDispatch: true,
		})
	})

	test("turns goal and plan selections into footer trigger chips", () => {
		expect({
			requirementComment: chatViewSource.includes(
				"Desktop slash commands are limited to first-party",
			),
			chipComponent: chatViewSource.includes("function ComposerTriggerChip"),
			footerChip: chatViewSource.includes("<ComposerTriggerChip"),
			goalIcon: chatViewSource.includes("GoalIcon"),
			planIcon: chatViewSource.includes("ListTodoIcon"),
			researchIcon: popoverSource.includes("MicroscopeIcon"),
			researchInsertText: popoverSource.includes('insertText: "/research "'),
			researchPromptPath: chatViewSource.includes('case "research":'),
			hoverBackground: chatViewSource.includes("hover:bg-muted"),
			shiftTabToggle: chatViewSource.includes('e.key === "Tab" && e.shiftKey'),
			triggeredPrompt: chatViewSource.includes("text: `/${trigger} ${text.trim()}`"),
		}).toEqual({
			requirementComment: true,
			chipComponent: true,
			footerChip: true,
			goalIcon: true,
			planIcon: true,
			researchIcon: true,
			researchInsertText: true,
			researchPromptPath: true,
			hoverBackground: true,
			shiftTabToggle: true,
			triggeredPrompt: true,
		})
	})
})
