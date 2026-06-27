import { appendFileSync, mkdirSync } from "node:fs"
import path from "node:path"

export const TRAFFIC_LOG_PATH_ENV = "TRAFFIC_LOG_PATH"

export type AcpTrafficDirection = "desktop-to-server" | "server-to-desktop" | "system"
export type AcpTrafficKind = "request" | "response" | "notification" | "invalid" | "closed"
export type AcpTrafficJsonRpcId = number | string

export interface AcpTrafficLogState {
	enabled: boolean
	path: string | null
}

export interface AcpTrafficLogRecord {
	direction: AcpTrafficDirection
	kind: AcpTrafficKind
	id?: AcpTrafficJsonRpcId
	method?: string
	payload?: unknown
}

export interface AcpTrafficLogger {
	getState(): AcpTrafficLogState
	record(entry: AcpTrafficLogRecord): void
}

interface CreateAcpTrafficLoggerOptions {
	env?: Record<string, string | undefined>
	clock?: () => Date
}

export function createAcpTrafficLoggerFromEnv({
	env = process.env,
	clock = () => new Date(),
}: CreateAcpTrafficLoggerOptions): AcpTrafficLogger {
	const logPath = env[TRAFFIC_LOG_PATH_ENV]?.trim() || null

	return new AcpTrafficFileLogger({ enabled: logPath !== null, path: logPath }, clock)
}

class AcpTrafficFileLogger implements AcpTrafficLogger {
	constructor(
		private readonly state: AcpTrafficLogState,
		private readonly clock: () => Date,
	) {}

	getState(): AcpTrafficLogState {
		return { ...this.state }
	}

	record(entry: AcpTrafficLogRecord): void {
		if (!this.state.enabled || !this.state.path) return

		mkdirSync(path.dirname(this.state.path), { recursive: true })
		appendFileSync(
			this.state.path,
			`${JSON.stringify({ timestamp: this.clock().toISOString(), ...entry })}\n`,
			"utf-8",
		)
	}
}
