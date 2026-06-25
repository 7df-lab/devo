import { describe, expect, mock, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { Agent } from "../../lib/types"

mock.module("@tanstack/react-router", () => ({
	useNavigate: () => () => undefined,
}))

const { SessionRow } = await import("./sidebar-rows")

function agent(): Agent {
	return {
		id: "session-1",
		sessionId: "session-1",
		name: "Greeting and Introduction",
		status: "idle",
		environment: "local",
		project: "devo",
		projectSlug: "devo-123",
		directory: "C:\\Users\\lenovo\\Desktop\\devo",
		projectDirectory: "C:\\Users\\lenovo\\Desktop\\devo",
		branch: "main",
		duration: "42m",
		activities: [],
		permissions: [],
		questions: [],
		createdAt: 1,
		lastActiveAt: 2,
	}
}

describe("SessionRow", () => {
	test("keeps the selected row visually stable on hover", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={agent()}
				isSelected
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasSelectedBackground: markup.includes("bg-black/[0.07]"),
			hasLightHoverBackground: markup.includes("hover:bg-black/[0.04]"),
			hasDarkHoverBackground: markup.includes("dark:hover:bg-white/[0.06]"),
			hidesStatusOnHover: markup.includes("group-hover/sidebar-row:opacity-0"),
			showsActionsOnHover: markup.includes("group-hover/sidebar-row:opacity-100"),
		}).toEqual({
			hasSelectedBackground: true,
			hasLightHoverBackground: false,
			hasDarkHoverBackground: false,
			hidesStatusOnHover: false,
			showsActionsOnHover: false,
		})
	})

	test("keeps hover affordances for unselected rows", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={agent()}
				isSelected={false}
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasLightHoverBackground: markup.includes("hover:bg-black/[0.04]"),
			hasDarkHoverBackground: markup.includes("dark:hover:bg-white/[0.06]"),
			hidesStatusOnHover: markup.includes("group-hover/sidebar-row:opacity-0"),
			showsActionsOnHover: markup.includes("group-hover/sidebar-row:opacity-100"),
		}).toEqual({
			hasLightHoverBackground: true,
			hasDarkHoverBackground: true,
			hidesStatusOnHover: true,
			showsActionsOnHover: true,
		})
	})

	test("renders last active text from timestamp instead of cached duration", () => {
		const originalNow = Date.now
		Date.now = () => Date.parse("2026-06-24T02:00:00.000Z")
		try {
			const staleAgent = {
				...agent(),
				duration: "now",
				lastActiveAt: Date.parse("2026-06-24T00:00:00.000Z"),
			}

			const markup = renderToStaticMarkup(
				<SessionRow
					agent={staleAgent}
					isSelected={false}
					onRename={async () => {}}
					onDelete={async () => {}}
					onFork={async () => {}}
				/>,
			)

			expect({
				usesTimestamp: markup.includes("2h"),
				usesCachedDuration: markup.includes(">now<"),
			}).toEqual({
				usesTimestamp: true,
				usesCachedDuration: false,
			})
		} finally {
			Date.now = originalNow
		}
	})
})
