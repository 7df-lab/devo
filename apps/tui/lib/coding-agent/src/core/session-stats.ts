import type { CumulativeUsage } from "./usage-display.js";
import type { ContextUsage } from "./extensions/index.js";

export interface SessionStats {
	sessionFile: string | undefined;
	sessionId: string;
	userMessages: number;
	assistantMessages: number;
	toolCalls: number;
	toolResults: number;
	totalMessages: number;
	tokens: CumulativeUsage["tokens"];
	cost: number | null;
	cumulativeUsage?: CumulativeUsage;
	contextUsage?: ContextUsage;
}
