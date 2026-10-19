import { describe, expect, test } from "bun:test"
import { PromptInputProvider } from "@devo/ui/components/ai-elements/prompt-input"
import { renderToStaticMarkup } from "react-dom/server"
import { ComposerSubmitControls } from "./composer-submit-controls"

describe("composer submit controls", () => {
	for (const scenario of [
		{ name: "idle", draft: "hello", working: false, connected: true, canSend: true, buttons: [{ label: "Submit", type: "submit", disabled: false }] },
		{ name: "working without a draft", draft: "  ", working: true, connected: true, canSend: true, buttons: [{ label: "Stop", type: "button", disabled: false }] },
		{ name: "working with a follow-up", draft: "follow up", working: true, connected: true, canSend: true, buttons: [{ label: "Stop", type: "button", disabled: false }, { label: "Queue message", type: "submit", disabled: false }] },
		{ name: "sending a follow-up", draft: "follow up", working: true, connected: true, canSend: false, buttons: [{ label: "Stop", type: "button", disabled: false }, { label: "Queue message", type: "submit", disabled: true }] },
		{ name: "disconnected", draft: "follow up", working: true, connected: false, canSend: false, buttons: [{ label: "Stop", type: "button", disabled: true }, { label: "Queue message", type: "submit", disabled: true }] },
	]) {
		test(scenario.name, () => {
			const html = renderToStaticMarkup(
				<PromptInputProvider initialInput={scenario.draft}>
					<ComposerSubmitControls isWorking={scenario.working} isConnected={scenario.connected} canSend={scenario.canSend} onStop={() => undefined} />
				</PromptInputProvider>,
			)
			const buttons = [...html.matchAll(/<button\b[^>]*>/g)].map(([tag]) => ({
				label: tag.match(/aria-label="([^"]+)"/)?.[1],
				type: tag.match(/type="([^"]+)"/)?.[1],
				disabled: /\bdisabled=/.test(tag),
			}))
			expect(buttons).toEqual(scenario.buttons)
		})
	}
})
