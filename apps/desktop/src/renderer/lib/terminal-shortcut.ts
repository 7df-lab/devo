interface TerminalToggleShortcutEvent {
	key: string;
	metaKey: boolean;
	ctrlKey: boolean;
	altKey: boolean;
}

export function isTerminalToggleShortcut(
	event: TerminalToggleShortcutEvent,
): boolean {
	return (
		event.key.toLowerCase() === "j" &&
		(event.metaKey || event.ctrlKey) &&
		!event.altKey
	);
}
