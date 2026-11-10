import type { ContextTreeNode } from "../../../core/context-tree.js";
import type { CumulativeUsage } from "../../../core/usage-display.js";

/** Native totals include child/overhead spend; context always belongs to this session. */
export function formatCumulativeContext(root: ContextTreeNode, usage: CumulativeUsage): string {
	const number = (value: number | null) => value === null ? "unknown" : value.toLocaleString();
	const context = root.contextUsage;
	const lines = ["Context", ""];
	if (root.model) lines.push(`Model: ${root.model.provider}/${root.model.id}`, "");
	lines.push(context && context.tokens !== null && context.percent !== null
		? `Current: ${number(context.tokens)} / ${number(context.contextWindow)} (${Math.round(context.percent * 10) / 10}%)`
		: "Current: unknown");
	lines.push("", "Session usage (including child agents and overhead)",
		`Input: ${number(usage.tokens.input)}`,
		`Output: ${number(usage.tokens.output)}`,
		`Cache Read: ${number(usage.tokens.cacheRead)}`,
		`Cache Write: ${number(usage.tokens.cacheWrite)}`,
		`Total: ${number(usage.tokens.total)}`,
		"Cache tokens are included in input, not added to the total.",
		`Known estimated cost: ${usage.cost === null ? "unknown" : `${usage.currency} ${usage.cost.toFixed(4)}`}`);
	if (usage.callCount !== null && usage.meteredCallCount !== null) {
		lines.push(`Metered calls: ${usage.meteredCallCount} / ${usage.callCount}`);
	}
	if (usage.coverage === "partial") lines.push("Partial usage: some calls or token fields were not reported.");
	if (usage.unreportedFields?.length) lines.push("Unknown fields were not reported for every request.");
	if (usage.coverage === "unknown") lines.push("Usage coverage is unavailable for older records.");
	if (usage.coverage === "unreported") lines.push("Provider usage was not reported.");
	if (usage.coverage === "legacy") lines.push("Historical tokens are included; breakdown covers reported calls only.");
	if (root.children.length) {
		lines.push("", "Child agents (separate context windows)");
		for (const child of root.children) {
			lines.push(`${child.label}: ${child.status}; context and own usage unavailable`);
		}
	}
	return lines.join("\n");
}
