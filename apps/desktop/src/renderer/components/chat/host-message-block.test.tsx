import { expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { HostMessageBlock } from "./host-message-block"

for (const [source, label] of Object.entries({
	agent: "Agent message",
	backgroundTask: "Background notification",
	schedule: "Scheduled notification",
})) {
	test(`labels ${source} without human message controls`, () => {
		const html = renderToStaticMarkup(
			<HostMessageBlock
				text="Result ready"
				origin={{
					source,
					sender: "worker-1",
					eventId: "event-1",
					delivery: "steer",
				}}
			/>,
		)
		expect(html).toContain(label)
		expect(html).toContain("worker-1")
		expect(html).toContain("Result ready")
		expect(html).not.toContain("button")
		expect(html).not.toContain("textarea")
	})
}

import { ComposerStatusStack } from "./composer-status-stack"

test("host queue rows keep their label and remove control but not edit or steer", () => {
	const html = renderToStaticMarkup(
		<ComposerStatusStack
			queueItems={[
				{
					id: "host-queue",
					text: "Result ready",
					status: "queued",
					origin: {
						eventId: "e",
						source: "agent",
						sender: "worker-1",
						delivery: "followUp",
					},
				},
			]}
		/>,
	)
	expect(html).toContain("Agent message")
	expect(html).toContain("worker-1")
	expect(html).not.toContain("Edit queued message")
	expect(html).not.toContain(">Steer<")
	expect(html).toContain("Remove queued message")
})

test("human queue rows retain edit and steer controls", () => {
	const html = renderToStaticMarkup(
		<ComposerStatusStack
			queueItems={[{ id: "human-queue", text: "Hello", status: "queued" }]}
		/>,
	)
	expect(html).toContain("Edit queued message")
	expect(html).toContain("Steer")
	expect(html).not.toContain("Agent message")
})
