import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import {
	TRAFFIC_LOG_PATH_ENV,
	createAcpTrafficLoggerFromEnv,
} from "./acp-traffic-log"

function withTempDir<T>(run: (dir: string) => T): T {
	const dir = mkdtempSync(path.join(tmpdir(), "devo-acp-traffic-log-"))
	try {
		return run(dir)
	} finally {
		rmSync(dir, { recursive: true, force: true })
	}
}

const fixedClock = () => new Date("2026-06-27T01:02:03.004Z")

describe("ACP traffic log env trigger", () => {
	test("does not create or write a log when TRAFFIC_LOG_PATH is unset or empty", () => {
		withTempDir((dir) => {
			const logger = createAcpTrafficLoggerFromEnv({
				env: {},
				clock: fixedClock,
			})

			logger.record({
				direction: "desktop-to-server",
				kind: "request",
				id: 1,
				method: "initialize",
				payload: { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
			})

			expect({
				state: logger.getState(),
				logsDirExists: existsSync(path.join(dir, "logs")),
			}).toEqual({
				state: { enabled: false, path: null },
				logsDirExists: false,
			})
		})

		withTempDir((dir) => {
			const logger = createAcpTrafficLoggerFromEnv({
				env: { [TRAFFIC_LOG_PATH_ENV]: " " },
				clock: fixedClock,
			})

			logger.record({
				direction: "system",
				kind: "closed",
				payload: { error: "transport closed" },
			})

			expect({
				state: logger.getState(),
				logsDirExists: existsSync(path.join(dir, "logs")),
			}).toEqual({
				state: { enabled: false, path: null },
				logsDirExists: false,
			})
		})
	})

	test("ignores the removed DEVO_DESKTOP_ACP_TRAFFIC_LOG trigger", () => {
		withTempDir((dir) => {
			const logger = createAcpTrafficLoggerFromEnv({
				env: {
					DEVO_DESKTOP_ACP_TRAFFIC_LOG: "1",
					DEVO_DESKTOP_ACP_TRAFFIC_LOG_PATH: path.join(dir, "old", "traffic.jsonl"),
				},
				clock: fixedClock,
			})

			logger.record({
				direction: "desktop-to-server",
				kind: "request",
				id: 1,
				method: "initialize",
				payload: { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
			})

			expect({
				state: logger.getState(),
				logsDirExists: existsSync(path.join(dir, "old")),
			}).toEqual({
				state: { enabled: false, path: null },
				logsDirExists: false,
			})
		})
	})

	test("writes JSONL when TRAFFIC_LOG_PATH is provided", () => {
		withTempDir((dir) => {
			const logPath = path.join(dir, "custom", "traffic.jsonl")
			const logger = createAcpTrafficLoggerFromEnv({
				env: {
					[TRAFFIC_LOG_PATH_ENV]: ` ${logPath} `,
				},
				clock: fixedClock,
			})

			logger.record({
				direction: "system",
				kind: "closed",
				payload: { error: "transport closed" },
			})

			const lines = readFileSync(logPath, "utf-8").trim().split("\n")
			expect({
				state: logger.getState(),
				entry: JSON.parse(lines[0]),
			}).toEqual({
				state: { enabled: true, path: logPath },
				entry: {
					timestamp: "2026-06-27T01:02:03.004Z",
					direction: "system",
					kind: "closed",
					payload: { error: "transport closed" },
				},
			})
		})
	})
})
