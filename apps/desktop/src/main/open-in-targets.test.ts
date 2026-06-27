import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const originalPlatform = process.platform
function setPlatform(value: NodeJS.Platform) {
	Object.defineProperty(process, "platform", { value, configurable: true })
}

let preferredTargetId: string | null = null
const updateSettingsCalls: unknown[] = []
const spawnCalls: Array<{ command: string; args: string[] }> = []
const fileIconCalls: string[] = []
let execFileSyncImpl: (command: string, args: string[]) => string = () => {
	throw new Error("command not found")
}

function currentSettings() {
	return {
		notifications: {
			completionMode: "unfocused",
			permissions: true,
			questions: true,
			errors: true,
			dockBadge: true,
		},
		opaqueWindows: false,
		appearance: {
			colorScheme: "dark",
			themeId: "default",
			rendererPreferencesMigrated: false,
		},
		openIn: {
			preferredTargetId,
		},
		servers: {
			servers: [{ id: "local", name: "This Mac", type: "local" }],
			activeServerId: "local",
		},
	}
}

mock.module("./settings-store", () => ({
	getSettings: currentSettings,
	updateSettings: (partial: { openIn?: { preferredTargetId?: string | null } }) => {
		updateSettingsCalls.push(partial)
		if (partial.openIn) {
			preferredTargetId = partial.openIn.preferredTargetId ?? null
		}
		return currentSettings()
	},
}))

mock.module("node:child_process", () => ({
	execFileSync: (command: string, args: string[]) => execFileSyncImpl(command, args),
	spawn: (command: string, args: string[]) => {
		spawnCalls.push({ command, args })
		const proc = {
			unref: () => {},
			on: (event: string, callback: () => void) => {
				if (event === "spawn") queueMicrotask(callback)
				return proc
			},
		}
		return proc
	},
}))

mock.module("electron", () => ({
	app: {
		getFileIcon: async (path: string) => {
			fileIconCalls.push(path)
			return {
				toPNG: () => Buffer.from("icon"),
			}
		},
	},
}))

beforeEach(() => {
	setPlatform("darwin")
	preferredTargetId = null
	updateSettingsCalls.length = 0
	spawnCalls.length = 0
	fileIconCalls.length = 0
	execFileSyncImpl = () => {
		throw new Error("command not found")
	}
})

afterAll(() => {
	setPlatform(originalPlatform)
})

async function loadOpenInTargets(name: string) {
	return await import(`./open-in-targets?case=${name}-${Date.now()}`)
}

describe("open-in-targets preferences", () => {
	test("persists the preferred open target in desktop settings", async () => {
		const { setPreferredTarget } = await loadOpenInTargets("persist")

		setPreferredTarget("cursor")

		expect(updateSettingsCalls).toEqual([
			{
				openIn: {
					preferredTargetId: "cursor",
				},
			},
		])
	})

	test("falls back to the first available target when the stored target is unavailable", async () => {
		const { resolvePreferredTargetId } = await loadOpenInTargets("fallback")

		expect(resolvePreferredTargetId("cursor", ["vscode", "finder"])).toBe("vscode")
	})

	test("detects Windows targets from PATH and resolves the preferred fallback", async () => {
		setPlatform("win32")
		preferredTargetId = "cursor"
		execFileSyncImpl = (command, args) => {
			if (command !== "where.exe") throw new Error(`unexpected command: ${command}`)
			const binary = args[0]
			const paths: Record<string, string[]> = {
				code: ["C:\\Tools\\code.cmd"],
				zed: ["C:\\Tools\\zed", "C:\\Tools\\zed.exe"],
				"explorer.exe": ["C:\\Windows\\explorer.exe"],
			}
			const matches = paths[binary]
			if (!matches) throw new Error(`not found: ${binary}`)
			return `${matches.join("\r\n")}\r\n`
		}
		const { getOpenInTargets } = await loadOpenInTargets("windows-detect")

		const result = await getOpenInTargets()
		const availableTargets = result.targets
			.filter((target: { available: boolean }) => target.available)
			.map((target: { id: string; label: string }) => ({ id: target.id, label: target.label }))

		expect({
			availableTargets,
			availableTargetIds: result.availableTargets,
			preferredTarget: result.preferredTarget,
		}).toEqual({
			availableTargets: [
				{ id: "vscode", label: "VS Code" },
				{ id: "zed", label: "Zed" },
				{ id: "finder", label: "File Explorer" },
			],
			availableTargetIds: ["vscode", "zed", "finder"],
			preferredTarget: "vscode",
		})
		expect(fileIconCalls).toEqual([
			"C:\\Tools\\code.cmd",
			"C:\\Tools\\zed.exe",
			"C:\\Windows\\explorer.exe",
		])
	})

	test("detects Windows VS Code from uninstall registry install location", async () => {
		setPlatform("win32")
		const installDir = mkdtempSync(join(tmpdir(), "devo-vscode-"))
		const codePath = join(installDir, "Code.exe")
		writeFileSync(codePath, "")
		try {
			execFileSyncImpl = (command, args) => {
				if (command === "where.exe") throw new Error(`not found: ${args[0]}`)
				if (command === "reg.exe" && args[0] === "query") {
					return [
						"HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\vscode",
						"    DisplayName    REG_SZ    Microsoft Visual Studio Code (User)",
						`    InstallLocation    REG_SZ    ${installDir}`,
						"",
					].join("\r\n")
				}
				throw new Error(`unexpected command: ${command}`)
			}
			const { getOpenInTargets } = await loadOpenInTargets("windows-registry-vscode")

			const result = await getOpenInTargets()

			expect(result.availableTargets).toContain("vscode")
			expect(fileIconCalls).toContain(codePath)
		} finally {
			rmSync(installDir, { recursive: true, force: true })
		}
	})

	test("wraps Windows cmd shims through cmd.exe", async () => {
		setPlatform("win32")
		execFileSyncImpl = (command, args) => {
			if (command === "where.exe" && args[0] === "code") return "C:\\Tools\\code.cmd\r\n"
			throw new Error("command not found")
		}
		const { openInTarget } = await loadOpenInTargets("windows-cmd-shim")

		await openInTarget("C:\\repo", "vscode")

		expect(spawnCalls).toEqual([
			{
				command: "cmd.exe",
				args: ["/d", "/s", "/c", "C:\\Tools\\code.cmd", "--goto", "C:\\repo"],
			},
		])
	})

	test("opens Windows projects in File Explorer", async () => {
		setPlatform("win32")
		execFileSyncImpl = (command, args) => {
			if (command === "where.exe" && args[0] === "explorer.exe") {
				return "C:\\Windows\\explorer.exe\r\n"
			}
			throw new Error("command not found")
		}
		const { openInTarget } = await loadOpenInTargets("windows-file-explorer")

		await openInTarget("C:\\repo", "finder")

		expect(spawnCalls).toEqual([
			{
				command: "C:\\Windows\\explorer.exe",
				args: ["C:\\repo"],
			},
		])
	})
})
