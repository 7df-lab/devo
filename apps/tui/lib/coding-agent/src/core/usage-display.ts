/** Provider-reported cumulative spend, never context occupancy or a text estimate. */
export interface CumulativeUsage {
	tokens: {
		input: number | null;
		output: number | null;
		cacheRead: number | null;
		cacheWrite: number | null;
		total: number | null;
	};
	/** Known estimated cost only. Null means no price was reported. */
	cost: number | null;
	currency: string | null;
	/** Historical spend whose token split was never recorded. */
	legacyTokens?: number;
	unreportedFields?: string[];
	callCount: number | null;
	meteredCallCount: number | null;
	coverage: "complete" | "partial" | "unreported" | "legacy" | "unknown";
}
