import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ComposerStatusStack, type ComposerQueueItem } from "./composer-status-stack"

function render(item: Partial<ComposerQueueItem>) {
	return renderToStaticMarkup(<ComposerStatusStack
		queueItems={[{ id: "queue-1", text: "queued text", status: "queued", ...item }]}
		onEditQueueItem={() => {}}
		onRemoveQueueItem={() => {}}
		onSteerQueueItem={() => {}}
		onRetryQueueItem={() => {}}
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

	test("a held entry disables dragging every row without disabling ordinary steering", () => {
		const html = renderToStaticMarkup(<ComposerStatusStack
			queueItems={[
				{ id: "held", text: "held", status: "queued", recoveryRequired: true },
				{ id: "normal", text: "normal", status: "queued" },
			]}
			onSteerQueueItem={() => {}}
		/>)
		expect(html.match(/draggable="false"/g)?.length).toBe(2)
		expect(html).not.toContain('draggable="true"')
		expect(html).toContain(">Steer</button>")
	})

	test("held recovery warns of prior effects, disables editing and dragging, and preserves retry and discard", () => {
		const html = render({ recoveryRequired: true })
		expect(html).toContain("Prior effects may have occurred")
		expect(html).toContain(">Retry</button>")
		expect(html).toContain('draggable="false"')
		expect(html.match(/<button[^>]*aria-label="Edit queued message"[^>]*>/)?.[0]).toContain('disabled=""')
		expect(html.match(/<button[^>]*aria-label="Remove queued message"[^>]*>/)?.[0]).not.toContain('disabled=""')
	})
})
