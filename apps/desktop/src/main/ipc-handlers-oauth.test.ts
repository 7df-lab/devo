import { beforeEach, describe, expect, mock, test } from "bun:test"

type Deferred<T> = {
	promise: Promise<T>
	resolve: (value: T) => void
	reject: (error: Error) => void
}

function deferred<T>(): Deferred<T> {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((ok, fail) => {
		resolve = ok
		reject = fail
	})
	return { promise, resolve, reject }
}

type IpcHandler = (
	event: ReturnType<typeof sender>,
	request?: { providerId: string },
) => Promise<void> | boolean | void
const handlers = new Map<string, IpcHandler>()
const logins: Array<Deferred<{ access: string }>> = []
const writes: Array<{
	provider: string
	access: string
	gate: Deferred<void>
}> = []
let stored: Record<string, string> = {}
const updates: Array<{
	senderId: number
	channel: string
	update: { phase?: string; instructions?: string }
}> = []
const unused = () => {
	throw new Error("Unrelated IPC handler invoked")
}

mock.module("electron", () => ({
	app: { isPackaged: false },
	BrowserWindow: { getAllWindows: () => [] },
	dialog: {},
	ipcMain: {
		handle: (channel: string, handler: IpcHandler) => {
			handlers.set(channel, handler)
		},
		on: () => {},
	},
	nativeTheme: { on: () => {} },
	net: {},
	shell: {},
	systemPreferences: { on: () => {} },
}))
mock.module("./logger", () => ({
	createLogger: () => ({
		info: () => {},
		debug: () => {},
		warn: () => {},
		error: () => {},
	}),
}))
mock.module("./terminal-manager", () => ({
	desktopTerminalManager: { onData: () => {}, onExit: () => {} },
}))
mock.module("./settings-store", () => ({
	getOpaqueWindows: unused,
	getSettings: unused,
	onSettingsChanged: () => {},
	updateSettings: unused,
}))
mock.module("./provider-oauth", () => ({
	isDesktopOAuthProviderId: (id: string) =>
		["anthropic", "openai-codex"].includes(id),
	loginDesktopOAuth: () => {
		const gate = deferred<{ access: string }>()
		logins.push(gate)
		return gate.promise // Deliberately ignores abort: tests the race after the provider resolves.
	},
	credentialSetParamsFromDesktopOAuth: (
		provider: string,
		credential: { access: string },
	) => ({
		provider,
		kind: "oauth",
		access: credential.access,
	}),
}))
mock.module("./devo-manager", () => ({
	requestNative: (
		method: string,
		params: { provider: string; access: string },
	) => {
		if (method !== "credential/set")
			throw new Error(`Unexpected RPC: ${method}`)
		const gate = deferred<void>()
		writes.push({ provider: params.provider, access: params.access, gate })
		return gate.promise.then(() => {
			stored[params.provider] = params.access
		})
	},
	subscribeNative: () => {},
	ensureServer: unused,
	getNativeTrafficLogState: unused,
	getServerUrl: unused,
	isNativeConnected: unused,
	notifyNative: unused,
	respondNative: unused,
	restartServer: unused,
	stopServer: unused,
}))

// Avoid starting unrelated Electron services when registering IPC handlers.
const stubModules: Array<[string, string]> = [
	[
		"./automation",
		"acceptRun archiveRun createAutomation deleteAutomation getAutomation listAutomations listRuns markRunRead previewSchedule runNow updateAutomation",
	],
	["./credential-store", "deleteCredential getCredential storeCredential"],
	["./desktop-runtime-check", "checkDesktopRuntime"],
	["./desktop-folders", "createDesktopFolder statDesktopFolders"],
	[
		"./git-service",
		"applyChangesToLocal applyDiffTextToLocal checkout commitAll createBranch getDiffStat getGitRoot getRemoteUrl getStatus listBranches push stashAndCheckout stashPop",
	],
	[
		"./git-workspace-changes",
		"localWorkspaceChangesSummary localWorkspaceFilePatch",
	],
	["./liquid-glass", "getResolvedChromeTier resolveTitleBarOverlay"],
	["./rules-files", "createProjectAgentsMd listRuleFiles"],
	["./model-state", "readModelState updateModelRecent"],
	["./notifications", "dismissNotification updateBadgeCount"],
	[
		"./onboarding",
		"detectProviders executeMigration previewMigration restoreMigrationBackup scanProvider",
	],
	["./mcp-config", "openUserMcpConfigFile"],
	["./open-in-targets", "getOpenInTargets openInTarget setPreferredTarget"],
	["../shared/mcp-config", "MCP_CONFIG_OPEN_PATH"],
	[
		"../shared/native-ipc-error",
		"isSessionNotFoundError nativeIpcErrorEnvelope",
	],
	[
		"./updater",
		"checkForUpdates downloadUpdate getUpdateState installUpdate openReleasePage",
	],
]
for (const [path, names] of stubModules) {
	mock.module(path, () =>
		Object.fromEntries(names.split(" ").map((name) => [name, unused])),
	)
}

const { registerIpcHandlers } = await import("./ipc-handlers")
registerIpcHandlers()

function sender(id: number) {
	return {
		sender: {
			id,
			send: (
				channel: string,
				update: { phase?: string; instructions?: string },
			) => {
				updates.push({ senderId: id, channel, update })
			},
		},
	}
}
function handler(channel: string): IpcHandler {
	const found = handlers.get(channel)
	if (!found) throw new Error(`Missing IPC handler: ${channel}`)
	return found
}
function login(id: number, providerId = "anthropic"): Promise<void> {
	return handler("provider-oauth:login")(sender(id), {
		providerId,
	}) as Promise<void>
}
function cancel(id: number): boolean {
	return handler("provider-oauth:cancel")(sender(id)) as boolean
}
// Advance only the promise continuations (never use timers or a live provider).
async function flush(): Promise<void> {
	for (let i = 0; i < 5; i++) await Promise.resolve()
}

beforeEach(() => {
	logins.length = 0
	writes.length = 0
	updates.length = 0
	stored = {}
})

describe("Desktop OAuth credential cancellation", () => {
	test("a late OAuth response after cancel cannot start credential/set", async () => {
		const pending = login(1)
		expect(cancel(1)).toBe(true)
		logins[0].resolve({ access: "old" })
		await expect(pending).rejects.toThrow("Login cancelled")
		expect(writes).toHaveLength(0)
	})

	test("cancel during saving is refused; new login waits and then wins", async () => {
		const old = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		expect(writes.map(({ access }) => access)).toEqual(["old"])
		expect(updates).toContainEqual({
			senderId: 1,
			channel: "provider-oauth:update",
			update: { phase: "saving", instructions: "Saving credential..." },
		})
		expect(cancel(1)).toBe(false)
		const latest = login(1)
		await flush()
		expect(logins).toHaveLength(1)
		expect(writes.map(({ access }) => access)).toEqual(["old"])
		writes[0].gate.resolve()
		await old // Once saving starts, the prior login must not report cancellation.
		await flush()
		expect(logins).toHaveLength(2)
		logins[1].resolve({ access: "new" })
		await flush()
		expect(writes.map(({ access }) => access)).toEqual(["old", "new"])
		writes[1].gate.resolve()
		await latest
		expect(stored.anthropic).toBe("new")
	})

	test("cross-window writes to one provider are serialized", async () => {
		const old = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		expect(cancel(1)).toBe(false)
		const latest = login(2)
		logins[1].resolve({ access: "new" })
		await flush()
		expect(writes).toHaveLength(1)
		writes[0].gate.resolve()
		await old
		await flush()
		writes[1].gate.resolve()
		await latest
		expect(stored.anthropic).toBe("new")
	})

	test("a canceled queued login cannot write after the first set completes", async () => {
		const old = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		const queued = login(1)
		await flush()
		expect(logins).toHaveLength(1)
		expect(cancel(1)).toBe(false)
		const queuedError = queued.then(
			() => null,
			(error: Error) => error,
		)
		writes[0].gate.resolve()
		await old
		expect((await queuedError)?.message).toBe("Login cancelled")
		expect(writes.map(({ access }) => access)).toEqual(["old"])
	})

	test("cancel during pending RPC is refused and the save completes", async () => {
		const pending = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		expect(cancel(1)).toBe(false)
		writes[0].gate.resolve()
		await pending
		expect(stored.anthropic).toBe("old")
		expect(writes).toHaveLength(1)
	})

	test("cancel while queued behind another window prevents its credential/set", async () => {
		const first = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		const queued = login(2)
		logins[1].resolve({ access: "queued" })
		await flush()
		expect(writes).toHaveLength(1)
		expect(cancel(2)).toBe(true)
		const queuedError = queued.then(
			() => null,
			(error: Error) => error,
		)
		writes[0].gate.resolve()
		await first
		expect((await queuedError)?.message).toBe("Login cancelled")
		expect(writes.map(({ access }) => access)).toEqual(["old"])
	})

	test("failed commit releases the provider queue for the next login", async () => {
		const first = login(1)
		logins[0].resolve({ access: "old" })
		await flush()
		const latest = login(2)
		logins[1].resolve({ access: "new" })
		await flush()
		expect(writes).toHaveLength(1)
		expect(cancel(1)).toBe(false)
		writes[0].gate.reject(new Error("Server unavailable"))
		await expect(first).rejects.toThrow("Server unavailable")
		await flush()
		expect(writes.map(({ access }) => access)).toEqual(["old", "new"])
		writes[1].gate.resolve()
		await latest
		expect(stored.anthropic).toBe("new")
	})

	test("an unrelated provider is not blocked by a pending write", async () => {
		const first = login(1)
		logins[0].resolve({ access: "a" })
		await flush()
		const second = login(2, "openai-codex")
		logins[1].resolve({ access: "b" })
		await flush()
		expect(writes.map(({ provider }) => provider)).toEqual([
			"anthropic",
			"openai-codex",
		])
		writes[1].gate.resolve()
		await second
		writes[0].gate.resolve()
		await first
	})
})
