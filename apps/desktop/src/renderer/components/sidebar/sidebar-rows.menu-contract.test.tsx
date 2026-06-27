import { describe, expect, mock, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { ReactNode } from "react"
import type { Agent } from "../../lib/types"

type MockMenuProps = {
	children?: ReactNode
	render?: ReactNode
	onClick?: unknown
	onSelect?: unknown
	variant?: string
}

function handlerFlag(handler: unknown): "true" | "false" {
	return typeof handler === "function" ? "true" : "false"
}

function MockMenuItem({ children, onClick, onSelect, variant = "default" }: MockMenuProps) {
	return (
		<div
			data-mock-menu-item={variant}
			data-has-on-click={handlerFlag(onClick)}
			data-has-on-select={handlerFlag(onSelect)}
		>
			{children}
		</div>
	)
}

function MockPassthrough({ children, render }: MockMenuProps) {
	return <>{render ?? children}</>
}

mock.module("@tanstack/react-router", () => ({
	useNavigate: () => () => undefined,
}))

mock.module("@devo/ui/components/dropdown-menu", () => ({
	DropdownMenu: MockPassthrough,
	DropdownMenuContent: MockPassthrough,
	DropdownMenuGroup: MockPassthrough,
	DropdownMenuItem: MockMenuItem,
	DropdownMenuSeparator: () => <div data-mock-menu-separator />,
	DropdownMenuTrigger: MockPassthrough,
}))

mock.module("@devo/ui/components/context-menu", () => ({
	ContextMenu: MockPassthrough,
	ContextMenuContent: MockPassthrough,
	ContextMenuItem: MockMenuItem,
	ContextMenuSeparator: () => <div data-mock-context-menu-separator />,
	ContextMenuTrigger: MockPassthrough,
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
		directory: "/Users/tsiao/Desktop/devo",
		projectDirectory: "/Users/tsiao/Desktop/devo",
		branch: "main",
		duration: "42m",
		activities: [],
		permissions: [],
		questions: [],
		createdAt: 1,
		lastActiveAt: 2,
	}
}

describe("SessionRow menu action contract", () => {
	test("wires menu actions through click handlers accepted by Base UI menu items", () => {
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
			hasDeleteAction: markup.includes("Delete"),
			deleteActionUsesClick: markup.includes(
				'data-mock-menu-item="destructive" data-has-on-click="true" data-has-on-select="false"',
			),
			anyActionUsesSelect: markup.includes('data-has-on-select="true"'),
		}).toEqual({
			hasDeleteAction: true,
			deleteActionUsesClick: true,
			anyActionUsesSelect: false,
		})
	})
})
