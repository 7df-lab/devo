export type ShortcutToken = "mod" | "shift" | string

function currentPlatform(): string | undefined {
	if (typeof window === "undefined" || !("devo" in window)) return undefined
	return window.devo.platform
}

export function formatShortcut(
	tokens: readonly ShortcutToken[],
	platform = currentPlatform(),
): string {
	const isMac = platform === "darwin"
	const parts = orderShortcutTokens(tokens, isMac).map((token) => formatShortcutToken(token, isMac))
	return isMac ? parts.join("") : parts.join("+")
}

function orderShortcutTokens(tokens: readonly ShortcutToken[], isMac: boolean): ShortcutToken[] {
	const modifierOrder = isMac ? ["shift", "mod"] : ["mod", "shift"]
	const modifiers = modifierOrder.filter((modifier) =>
		tokens.some((token) => token.toLowerCase() === modifier),
	)
	const rest = tokens.filter((token) => !modifierOrder.includes(token.toLowerCase()))
	return [...modifiers, ...rest]
}

function formatShortcutToken(token: ShortcutToken, isMac: boolean): string {
	switch (token.toLowerCase()) {
		case "mod":
			return isMac ? "⌘" : "Ctrl"
		case "shift":
			return isMac ? "⇧" : "Shift"
		default:
			return token.length === 1 ? token.toUpperCase() : token
	}
}
