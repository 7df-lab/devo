import type { AcpTransport, AcpTransportEvent, AcpTransportListener, JsonRpcId } from "./acp-stdio-client"
import { app } from "electron"
import { StdioAcpClient, SUPPRESS_SERVER_TRAY_ENV } from "./acp-stdio-client"
import { resolveDevoProgram } from "./devo-program"
import { createLogger } from "./logger"
import { startNotificationWatcher, stopNotificationWatcher } from "./notification-watcher"
import { waitForEnv } from "./shell-env"

const log = createLogger("devo-manager")

const STDIO_URL = "stdio://local"

export interface DevoServer {
	url: string
	transport: "stdio"
	pid: number | null
	managed: boolean
}

let stdioClient: StdioAcpClient | null = null
let server: DevoServer | null = null
let initializing: Promise<DevoServer> | null = null

export async function ensureServer(): Promise<DevoServer> {
	if (server && stdioClient?.connected()) return server
	if (initializing) return initializing

	initializing = startServer().finally(() => {
		initializing = null
	})
	return initializing
}

export function getServerUrl(): string | null {
	return server?.url ?? null
}

export function stopServer(): boolean {
	stopNotificationWatcher()
	const hadClient = stdioClient !== null
	stdioClient?.stop()
	stdioClient = null
	server = null
	return hadClient
}

export async function restartServer(): Promise<DevoServer> {
	stopServer()
	return ensureServer()
}

export async function requestAcp(method: string, params?: unknown): Promise<unknown> {
	const client = await ensureClient()
	return client.request(method, params)
}

export async function respondAcp(id: JsonRpcId, result: unknown): Promise<void> {
	const client = await ensureClient()
	await client.respond(id, result)
}

export function subscribeAcp(listener: AcpTransportListener): () => void {
	const client = getOrCreateClient()
	return client.subscribe(listener)
}

export function isAcpConnected(): boolean {
	return stdioClient?.connected() ?? false
}

export function getAcpTransport(): AcpTransport {
	return {
		request: requestAcp,
		respond: respondAcp,
		subscribe: subscribeAcp,
		connected: isAcpConnected,
		pid: () => stdioClient?.pid() ?? null,
		stop: stopServer,
	}
}

async function startServer(): Promise<DevoServer> {
	await waitForEnv()
	const client = getOrCreateClient()
	client.start()

	await initialize(client)

	server = {
		url: STDIO_URL,
		transport: "stdio",
		pid: client.pid(),
		managed: true,
	}
	startNotificationWatcher(getAcpTransport())
	log.info("Devo ACP stdio server ready", { pid: server.pid })
	return server
}

async function ensureClient(): Promise<StdioAcpClient> {
	await ensureServer()
	return getOrCreateClient()
}

function getOrCreateClient(): StdioAcpClient {
	if (!stdioClient) {
		const program = resolveDevoProgram({
			appPath: app.getAppPath(),
			env: process.env,
			isPackaged: app.isPackaged,
		})
		stdioClient = new StdioAcpClient({
			program,
			env: { [SUPPRESS_SERVER_TRAY_ENV]: "1" },
		})
		stdioClient.subscribe(handleTransportEvent)
	}
	return stdioClient
}

function handleTransportEvent(event: AcpTransportEvent): void {
	if (event.type === "closed") {
		log.warn("Devo ACP stdio transport closed", { error: event.error })
		server = null
	}
}

async function initialize(client: StdioAcpClient): Promise<void> {
	await client.request("initialize", {
		protocolVersion: 1,
		clientCapabilities: {
			fs: { readTextFile: false, writeTextFile: false },
			terminal: false,
		},
		clientInfo: {
			name: "devo-desktop",
			title: "Devo Desktop",
			version: "0.1.0",
		},
	})
}
