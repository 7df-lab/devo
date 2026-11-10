import { useEffect } from "react"

/** Only the visible transcript owns session undo/redo; editors retain native history. */
export function useSessionHistoryShortcuts({
	isActive,
	canUndo,
	canRedo,
	onUndo,
	onRedo,
}: {
	isActive: boolean
	canUndo?: boolean
	canRedo?: boolean
	onUndo?: () => Promise<string | undefined>
	onRedo?: () => Promise<void>
}) {
	useEffect(() => {
		if (!isActive) return

		const handleKeyDown = (event: KeyboardEvent) => {
			if (
				event.defaultPrevented ||
				event.isComposing ||
				event.keyCode === 229 ||
				!(event.metaKey || event.ctrlKey) ||
				event.altKey ||
				event.key.toLowerCase() !== "z"
			)
				return

			// Keep native undo/redo inside inputs and rich text editors.
			const target = event.target
			if (
				target instanceof Element &&
				target.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')
			)
				return

			if (event.shiftKey) {
				if (canRedo && onRedo) {
					event.preventDefault()
					void onRedo()
				}
			} else if (canUndo && onUndo) {
				event.preventDefault()
				void onUndo()
			}
		}

		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [isActive, canUndo, canRedo, onUndo, onRedo])
}
