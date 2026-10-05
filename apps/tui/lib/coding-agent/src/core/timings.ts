/**
 * Central timing instrumentation for TUI startup profiling.
 * Enable with PI_TIMING=1 environment variable. Records stage names and durations only.
 */

import { performance } from "node:perf_hooks";
import { getLogger } from "@earendil-works/pi-ai";

const log = getLogger("coding-agent.timings");
const ENABLED = process.env.PI_TIMING === "1";
const timings: Array<{ label: string; ms: number }> = [];
let timingEpoch = 0;
let lastTime = performance.now();

export function resetTimings(): void {
	if (!ENABLED) return;
	timingEpoch++;
	timings.length = 0;
	lastTime = performance.now();
}

export function time(label: string): void {
	if (!ENABLED) return;
	const now = performance.now();
	timings.push({ label, ms: Number((now - lastTime).toFixed(1)) });
	lastTime = now;
}

export function recordDuration(label: string, startedAt: number, epoch = timingEpoch): void {
	if (!ENABLED || epoch !== timingEpoch) return;
	timings.push({ label, ms: Number((performance.now() - startedAt).toFixed(1)) });
}

export async function measureAsync<T>(label: string, operation: () => Promise<T>): Promise<T> {
	if (!ENABLED) return operation();
	const epoch = timingEpoch;
	const startedAt = performance.now();
	try {
		return await operation();
	} finally {
		recordDuration(label, startedAt, epoch);
	}
}

export function printTimings(): void {
	if (!ENABLED || timings.length === 0) return;
	const totalMs = timings.reduce((a, b) => a + b.ms, 0);
	log.debug("startup timings", { timings: [...timings], totalMs });
	console.error("\n--- TUI Stage Timings (overlapping stages may double-count) ---");
	for (const t of timings) {
		console.error(`  ${t.label}: ${t.ms}ms`);
	}
	console.error(`  SUM of stage durations: ${totalMs}ms`);
	console.error("------------------------\n");
	timings.length = 0;
}
