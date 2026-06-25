export interface TerminalToggleInput {
	alt: boolean;
	control: boolean;
	key: string;
	meta: boolean;
	type: string;
}

export function isTerminalToggleInput(input: TerminalToggleInput): boolean {
	return (
		input.type === "keyDown" &&
		input.key.toLowerCase() === "j" &&
		(input.meta || input.control) &&
		!input.alt
	);
}
