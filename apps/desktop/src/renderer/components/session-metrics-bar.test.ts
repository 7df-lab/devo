import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./session-metrics-bar.tsx", import.meta.url), "utf8")
const agentDetailSource = readFileSync(new URL("./agent-detail.tsx", import.meta.url), "utf8")

describe("SessionMetricsBar top timer wiring", () => {
	test("uses current chat turns for the inline timer instead of cumulative session work time", () => {
		expect({
			acceptsTurnsProp: source.includes("turns: ChatTurn[]"),
			acceptsIsWorkingProp: source.includes("isWorking: boolean"),
			usesLatestTurnTimer: source.includes("computeLatestTurnTimerSplit(turns"),
			omitsCompletedSessionWorkTime: !source.includes("completedMs={metrics.completedWorkTimeMs}"),
			headerPassesTurns: agentDetailSource.includes("turns={chatTurns}"),
			headerPassesWorkingState: agentDetailSource.includes('isWorking={agent.status === "running"}'),
		}).toEqual({
			acceptsTurnsProp: true,
			acceptsIsWorkingProp: true,
			usesLatestTurnTimer: true,
			omitsCompletedSessionWorkTime: true,
			headerPassesTurns: true,
			headerPassesWorkingState: true,
		})
	})
})
