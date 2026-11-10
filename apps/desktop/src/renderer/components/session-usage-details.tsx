import type { SessionUsageSummary } from "@devo-ai/sdk/v2/client"
import { formatTokens } from "../lib/session-metrics"

/** Session spend is cumulative across requests, not the current context window. */
export function SessionUsageDetails({ usage }: { usage?: SessionUsageSummary }) {
	const fields = [
		["Input", usage?.inputTokens], ["Output", usage?.outputTokens],
		["Reasoning", usage?.reasoningTokens], ["Cache read", usage?.cacheReadInputTokens],
		["Cache write", usage?.cacheCreationInputTokens], ["Total", usage?.totalTokens],
	] as const
	const coverage = !usage ? "No usage reported yet."
		: usage.coverage === "legacy" ? "Includes historical totals. Breakdown covers reported requests only."
			: usage.coverage === "unreported" ? "The provider has not reported token usage."
			: usage.coverage === "partial" ? `Reported tokens only: ${usage.meteredCallCount} of ${usage.callCount} requests metered.`
				: usage.coverage === "unknown" ? "Usage coverage is unavailable for older records."
					: `${usage.callCount} ${usage.callCount === 1 ? "request" : "requests"} metered.`
	return (
		<section aria-label="Cumulative token usage">
			<p className="mb-1.5 font-medium text-foreground/80">Session token usage</p>
			<dl className="grid grid-cols-2 gap-x-3 gap-y-0.5 text-muted-foreground">
				{fields.map(([label, value]) => (
					<div className="contents" key={label}>
						<dt>{label}</dt>
						<dd className="text-right tabular-nums">{value == null ? "Unknown" : formatTokens(value)}</dd>
					</div>
				))}
			</dl>
			<p className="mt-1 text-muted-foreground">{coverage}</p>
			{usage?.unreportedFields?.length ? <p className="mt-1 text-muted-foreground">Some token fields were not reported for every request.</p> : null}
			<p className="mt-1 text-muted-foreground">Cache tokens are included in input.</p>
		</section>
	)
}
