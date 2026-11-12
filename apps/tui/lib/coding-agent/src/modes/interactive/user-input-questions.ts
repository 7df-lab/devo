import type { AgentConnectionExtensionUiResponse } from "../agent-connection/types.js";

interface QuestionDialogs {
	select(title: string, options: string[]): Promise<string | undefined>;
	input(title: string): Promise<string | undefined>;
}

/** Collect the complete Native question set before answering its single reverse RPC. */
export async function resolveUserInputQuestions(
	value: unknown,
	dialogs: QuestionDialogs,
	signal?: AbortSignal,
): Promise<AgentConnectionExtensionUiResponse> {
	if (!Array.isArray(value) || value.length === 0) throw new Error("User input request contains no questions");
	const answers: Record<string, { answers: string[] }> = Object.create(null);
	for (const question of value) {
		if (!question || typeof question.id !== "string" || !question.id || typeof question.question !== "string") {
			throw new Error("Invalid user input question");
		}
		// The standard extension input is not masked. Fail closed rather than
		// accidentally echoing a secret into the terminal/scrollback.
		if (question.isSecret) throw new Error("Secret user input is not supported in the TUI; use a secure input channel");
		if (Object.hasOwn(answers, question.id)) throw new Error("Duplicate user input question id");
		if (signal?.aborted) return { cancelled: true };
		const options: Array<{ label: string; description?: string }> = Array.isArray(question.options) ? question.options : [];
		if (options.some((option) => !option || typeof option.label !== "string")) {
			throw new Error("Invalid user input options");
		}
		let answer: string | undefined;
		if (options.length > 0) {
			// Number the display labels so duplicate labels/descriptions and an option
			// named Other cannot collide. Send the original label, never decoration.
			const labels = options.map((option, index) => `${index + 1}. ${option.label}${option.description ? ` — ${option.description}` : ""}`);
			const other = `${options.length + 1}. Other (write an answer)`;
			const selected = await dialogs.select(question.question, question.isOther ? [...labels, other] : labels);
			if (signal?.aborted || selected === undefined) return { cancelled: true };
			if (question.isOther && selected === other) answer = await dialogs.input(question.question);
			else answer = options[labels.indexOf(selected)]?.label;
		} else {
			answer = await dialogs.input(question.question);
		}
		if (signal?.aborted || answer === undefined) return { cancelled: true };
		answers[question.id] = { answers: [answer] };
	}
	return { answers };
}
