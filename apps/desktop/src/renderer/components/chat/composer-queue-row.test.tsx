import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ComposerStatusStack, type ComposerQueueItem } from "./composer-status-stack"

function render(item: Partial<ComposerQueueItem>) {
	return renderToStaticMarkup(<ComposerStatusStack
		queueItems={[{ id: "queue-1", text: "queued text", status: "queued", ...item }]}
		onEditQueueItem={() => {}}
		onRemoveQueueItem={() => {}}
		onSteerQueueItem={() => {}}
	/>)
}

describe("composer queue row", () => {
	test("announces action errors while keeping queued actions available for retry", () => {
		const html = render({ error: "Connection unavailable" })
		expect(html).toContain('role="alert"')
		expect(html).toContain("Connection unavailable")
		expect(html).not.toContain('disabled=""')
	})

	test("prevents text editing from dropping skills or other non-text context", () => {
		const html = render({ canEdit: false })
		const editButton = html.match(/<button[^>]*aria-label="Edit queued message"[^>]*>/)?.[0]
		expect(editButton).toContain('disabled=""')
	})

	test("labels a successful steer with failed cleanup as removal-only retry", () => {
		const html = render({ steered: true, canEdit: false, error: "Remove unavailable" })
		expect(html).toContain("Retry removal")
		expect(html).not.toContain(">Steer<")
	})
})
