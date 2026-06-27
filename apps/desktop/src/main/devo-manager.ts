import type { AcpTransport, AcpTransportEvent, AcpTransportListener, JsonRpcId } from "./acp-stdio-client"
import { app } from "electron"
import {
	TRAFFIC_LOG_PATH_ENV,
	createAcpTrafficLoggerFromEnv,
	type AcpTrafficLogger,
	type AcpTrafficLogState,
} from "./acp-traffic-log"
import { StdioAcpClient, SUPPRESS_SERVER_TRAY_ENV } from "./acp-stdio-client"
import { resolveDevoProgram } from "./devo-program"
import { createLogger } from "./logger"
import { startNotificationWatcher, stopNotificationWatcher } from "./notification-watcher"
import { getSettings } from "./settings-store"
import { waitForEnv } from "./shell-env"

const log = createLogger("devo-manager")

const STDIO_URL = "stdio://local"
const acpTrafficLogStartupEnv = {
	[TRAFFIC_LOG_PATH_ENV]: process.env[TRAFFIC_LOG_PATH_ENV],
}

export interface DevoServer {
	url: string
	transport: "stdio"
	pid: number | null
	managed: boolean
}

let stdioClient: StdioAcpClient | null = null
let server: DevoServer | null = null
let initializing: Promise<DevoServer> | null = null
let acpTrafficLogger: AcpTrafficLogger | null = null
const serverReadyListeners = new Set<() => void>()

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

export function onServerReady(listener: () => void): () => void {
	serverReadyListeners.add(listener)
	if (server && stdioClient?.connected()) {
		queueMicrotask(() => {
			if (serverReadyListeners.has(listener) && server && stdioClient?.connected()) {
				listener()
			}
		})
	}
	return () => {
		serverReadyListeners.delete(listener)
	}
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

export function getAcpTrafficLogState(): AcpTrafficLogState {
	return getAcpTrafficLogger().getState()
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
	notifyServerReady()
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
			resourcesPath: process.resourcesPath,
		})
			stdioClient = new StdioAcpClient({
				program,
				env: { [SUPPRESS_SERVER_TRAY_ENV]: "1" },
				networkProxy: getSettings().servers.networkProxy,
				trafficLogger: getAcpTrafficLogger(),
			})
		stdioClient.subscribe(handleTransportEvent)
	}
	return stdioClient
}

function getAcpTrafficLogger(): AcpTrafficLogger {
	if (!acpTrafficLogger) {
		acpTrafficLogger = createAcpTrafficLoggerFromEnv({
			env: acpTrafficLogStartupEnv,
		})
	}
	return acpTrafficLogger
}

function handleTransportEvent(event: AcpTransportEvent): void {
	if (event.type === "closed") {
		log.warn("Devo ACP stdio transport closed", { error: event.error })
		server = null
	}
}

function notifyServerReady(): void {
	for (const listener of serverReadyListeners) {
		try {
			listener()
		} catch (error) {
			log.warn("Server-ready listener failed", error)
		}
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
