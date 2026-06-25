import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process"
import { EventEmitter } from "node:events"
import { createInterface } from "node:readline"
import { homedir } from "node:os"
import path from "node:path"
import { createLogger } from "./logger"

const log = createLogger("acp-stdio-client")
export const SUPPRESS_SERVER_TRAY_ENV = "DEVO_SUPPRESS_SERVER_TRAY"

export type JsonRpcId = number | string

type PendingRequest = {
	resolve: (value: unknown) => void
	reject: (error: Error) => void
}

export type AcpIncomingMessage =
	| { type: "response"; id: JsonRpcId; message: Record<string, unknown> }
	| { type: "notification"; method: string; params: unknown }
	| { type: "request"; id: JsonRpcId; method: string; params: unknown }
	| { type: "invalid"; error: string; line: string }

export type AcpTransportEvent =
	| { type: "notification"; method: string; params: unknown }
	| { type: "request"; id: JsonRpcId; method: string; params: unknown }
	| { type: "closed"; error?: string }

export type AcpTransportListener = (event: AcpTransportEvent) => void

export interface AcpTransport {
	request(method: string, params?: unknown): Promise<unknown>
	respond(id: JsonRpcId, result: unknown): Promise<void>
	subscribe(listener: AcpTransportListener): () => void
	connected(): boolean
	pid(): number | null
	stop(): void
}

export interface StdioAcpClientOptions {
	program?: string
	args?: string[]
	cwd?: string
	env?: NodeJS.ProcessEnv
}

export interface BuildServerProcessEnvOptions {
	baseEnv?: NodeJS.ProcessEnv
	homeDir?: string
	optionsEnv?: NodeJS.ProcessEnv
	pathSeparator?: string
}

function toError(error: unknown): Error {
	return error instanceof Error ? error : new Error(String(error))
}

export function buildServerProcessEnv({
	baseEnv = process.env,
	homeDir = homedir(),
	optionsEnv = {},
	pathSeparator = process.platform === "win32" ? ";" : ":",
}: BuildServerProcessEnvOptions = {}): NodeJS.ProcessEnv {
	const devoBinDir = path.join(homeDir, ".devo", "bin")
	return {
		...baseEnv,
		...optionsEnv,
		PATH: `${devoBinDir}${pathSeparator}${optionsEnv.PATH ?? baseEnv.PATH ?? ""}`,
	}
}

export function routeAcpLine(line: string): AcpIncomingMessage {
	let message: Record<string, unknown>
	try {
		message = JSON.parse(line) as Record<string, unknown>
	} catch (error) {
		return { type: "invalid", error: error instanceof Error ? error.message : String(error), line }
	}

	const id = message.id
	const method = message.method

	if ((typeof id === "number" || typeof id === "string") && typeof method === "string") {
		return {
			type: "request",
			id,
			method,
			params: "params" in message ? message.params : {},
		}
	}

	if (typeof id === "number" || typeof id === "string") {
		return { type: "response", id, message }
	}

	if (typeof method === "string") {
		return {
			type: "notification",
			method,
			params: "params" in message ? message.params : {},
		}
	}

	return { type: "invalid", error: "JSON-RPC message has no id or method", line }
}

export class StdioAcpClient implements AcpTransport {
	private child: ChildProcessWithoutNullStreams | null = null
	private nextId = 1
	private pending = new Map<JsonRpcId, PendingRequest>()
	private events = new EventEmitter()
	private stopped = false

	constructor(private readonly options: StdioAcpClientOptions = {}) {}

	start(): void {
		if (this.child) return

		const devoBinDir = path.join(homedir(), ".devo", "bin")
		const env = buildServerProcessEnv({ optionsEnv: this.options.env })
		const program = this.options.program ?? "devo"
		const args = this.options.args ?? ["server", "--transport", "stdio"]
		const cwd = this.options.cwd ?? homedir()

		log.info("Spawning Devo ACP stdio server", { program, args, cwd, binDir: devoBinDir })
		const child = spawn(program, args, {
			cwd,
			env,
			stdio: "pipe",
		})
		this.child = child
		this.stopped = false

		const stdout = createInterface({ input: child.stdout })
		stdout.on("line", (line) => this.handleLine(line))

		const stderr = createInterface({ input: child.stderr })
		stderr.on("line", (line) => {
			if (line.trim()) log.warn(`[stderr] ${line}`)
		})

		child.stdin.on("error", (error) => {
			const reason = toError(error)
			log.warn("Devo ACP stdio stdin failed", reason)
			this.close(reason)
		})

		child.on("error", (error) => {
			log.error("Devo ACP stdio process failed", error)
			this.close(error)
		})

		child.on("exit", (code, signal) => {
			if (this.stopped) return
			const reason = `Devo ACP stdio process exited with code ${code ?? "null"} signal ${signal ?? "null"}`
			log.warn(reason)
			this.close(new Error(reason))
		})
	}

	async request(method: string, params: unknown = {}): Promise<unknown> {
		this.start()
		const id = this.nextId++
		const child = this.requireChild()
		const response = new Promise<unknown>((resolve, reject) => {
			this.pending.set(id, { resolve, reject })
		})
		try {
			await this.writeJson(child, { jsonrpc: "2.0", id, method, params })
		} catch (error) {
			const reason = toError(error)
			this.pending.delete(id)
			this.close(reason)
			throw reason
		}
		return response
	}

	async respond(id: JsonRpcId, result: unknown): Promise<void> {
		this.start()
		try {
			await this.writeJson(this.requireChild(), { jsonrpc: "2.0", id, result })
		} catch (error) {
			const reason = toError(error)
			this.close(reason)
			throw reason
		}
	}

	subscribe(listener: AcpTransportListener): () => void {
		this.events.on("event", listener)
		return () => this.events.off("event", listener)
	}

	connected(): boolean {
		return this.child !== null && !this.child.killed && !this.stopped
	}

	pid(): number | null {
		return this.child?.pid ?? null
	}

	stop(): void {
		this.stopped = true
		const child = this.child
		this.child = null
		if (child && !child.killed) child.kill()
		this.close(new Error("Devo ACP stdio client stopped"))
	}

	private handleLine(line: string): void {
		const routed = routeAcpLine(line)
		switch (routed.type) {
			case "response": {
				const pending = this.pending.get(routed.id)
				if (!pending) return
				this.pending.delete(routed.id)
				const error = routed.message.error as { message?: string } | undefined
				if (error) {
					pending.reject(new Error(error.message ?? "Devo ACP request failed"))
				} else {
					pending.resolve(routed.message.result)
				}
				break
			}
			case "notification":
				this.events.emit("event", {
					type: "notification",
					method: routed.method,
					params: routed.params,
				} satisfies AcpTransportEvent)
				break
			case "request":
				this.events.emit("event", {
					type: "request",
					id: routed.id,
					method: routed.method,
					params: routed.params,
				} satisfies AcpTransportEvent)
				break
			case "invalid":
				log.warn("Ignoring invalid ACP stdio line", { error: routed.error, line: routed.line })
				break
		}
	}

	private writeJson(child: ChildProcessWithoutNullStreams, value: unknown): Promise<void> {
		const line = `${JSON.stringify(value)}\n`
		if (child.stdin.destroyed || child.stdin.writableEnded || !child.stdin.writable) {
			return Promise.reject(new Error("Devo ACP stdio stdin is closed"))
		}

		return new Promise((resolve, reject) => {
			try {
				if (!child.stdin.write(line, (error) => {
					if (error) {
						reject(toError(error))
					} else {
						resolve()
					}
				})) {
					log.debug("ACP stdio stdin applying backpressure")
				}
			} catch (error) {
				reject(toError(error))
			}
		})
	}

	private requireChild(): ChildProcessWithoutNullStreams {
		if (!this.child) throw new Error("Devo ACP stdio process is not running")
		return this.child
	}

	private close(error: Error): void {
		this.child = null
		for (const pending of this.pending.values()) {
			pending.reject(error)
		}
		this.pending.clear()
		this.events.emit("event", { type: "closed", error: error.message } satisfies AcpTransportEvent)
	}
}
