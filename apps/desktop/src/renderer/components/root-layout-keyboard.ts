/** Session shortcuts must not override controls or an open portal overlay. */
const INTERACTIVE_TARGET =
	'input, textarea, select, button, a[href], [contenteditable]:not([contenteditable="false"]), [role="button"], [role="link"], [role="checkbox"], [role="switch"], [role="tab"], [role="slider"], [role="combobox"], [role="menuitem"], [role="option"], [aria-label="Tool permission request"]'

const OPEN_OVERLAY =
	'dialog[open], [role="dialog"][data-open], [role="alertdialog"][data-open], [role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"], [data-slot="popover-content"][data-open], [data-slot="dropdown-menu-content"][data-open], [data-slot="context-menu-content"][data-open], [data-slot="select-content"][data-open], [data-slot="combobox-content"][data-open], [role="menu"][data-open]'

function isInteractive(element: EventTarget | null): boolean {
	if (!(element instanceof Element)) return false
	return (
		element.closest(INTERACTIVE_TARGET) !== null ||
		(element instanceof HTMLElement && element.isContentEditable)
	)
}

export function hasOtherOpenOverlay(ignoreSelector?: string): boolean {
	return Array.from(document.querySelectorAll(OPEN_OVERLAY)).some(
		(overlay) => !ignoreSelector || !overlay.matches(ignoreSelector),
	)
}

/** The palette's own dialog may close on Cmd/Ctrl+K; unrelated overlays may not. */
export function shouldToggleCommandPalette(event: KeyboardEvent, open: boolean): boolean {
	return (
		event.key === "k" &&
		(event.metaKey || event.ctrlKey) &&
		!event.isComposing &&
		!hasOtherOpenOverlay(open ? '[data-slot="dialog-content"].devo-command-palette' : undefined)
	)
}

export function isSessionNavigationBlocked(event: KeyboardEvent): boolean {
	return (
		event.defaultPrevented ||
		event.isComposing ||
		isInteractive(event.target) ||
		isInteractive(document.activeElement) ||
		hasOtherOpenOverlay()
	)
}

/** Return true only if an unblocked Escape/j/k shortcut was handled. */
export function handleSessionNavigationKeyDown(
	event: KeyboardEvent,
	options: {
		agents: ReadonlyArray<{ id: string; projectSlug: string }>
		sessionId: string | undefined
		navigate: (target: { to: string; params?: { projectSlug: string; sessionId: string } }) => void
	},
): boolean {
	const { agents, sessionId, navigate } = options
	if (event.key !== "Escape" && event.key !== "j" && event.key !== "k") return false
	if (isSessionNavigationBlocked(event)) return false

	if (event.key === "Escape") {
		event.preventDefault()
		navigate({ to: "/" })
		return true
	}

	if (event.metaKey || event.ctrlKey || event.altKey) return false
	event.preventDefault()
	const currentIndex = agents.findIndex((agent) => agent.id === sessionId)
	const nextIndex =
		event.key === "j"
			? currentIndex < agents.length - 1
				? currentIndex + 1
				: 0
			: currentIndex > 0
				? currentIndex - 1
				: agents.length - 1
	const agent = agents[nextIndex]
	if (agent) {
		navigate({
			to: "/project/$projectSlug/session/$sessionId",
			params: { projectSlug: agent.projectSlug, sessionId: agent.id },
		})
	}
	return true
}
