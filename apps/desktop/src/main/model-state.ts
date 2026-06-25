import { access, mkdir, readFile, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"

// ============================================================
// Types
// ============================================================

interface ModelRef {
	providerID: string
	modelID: string
}

interface ModelState {
	recent: ModelRef[]
	favorite: ModelRef[]
	variant: Record<string, string | undefined>
}

// ============================================================
// Helpers
// ============================================================

const EMPTY_STATE: ModelState = { recent: [], favorite: [], variant: {} }
const MAX_RECENT = 10

/**
 * Resolves the Devo state directory path.
 * Queries the running server first, falls back to default XDG path.
 */
async function resolveStatePath(): Promise<string> {
	try {
		const pathRes = await fetch("http://127.0.0.1:4101/path", {
			signal: AbortSignal.timeout(2000),
		})
		if (pathRes.ok) {
			const paths = (await pathRes.json()) as { state: string }
			return paths.state
		}
	} catch {
		// Server unreachable — fall through
	}
	return join(homedir(), ".local", "state", "devo")
}

// ============================================================
// Read model state
// ============================================================

/**
 * Reads the Devo model state (recent models, favorites, variants).
 *
 * First discovers the state directory by querying the running Devo server,
 * then reads `{state}/model.json`.
 * Falls back to the default XDG path if the server is unreachable.
 */
export async function readModelState(): Promise<ModelState> {
	try {
		const statePath = await resolveStatePath()
		const modelFile = join(statePath, "model.json")

		// Check if file exists
		try {
			await access(modelFile)
		} catch {
			return EMPTY_STATE
		}

		const content = await readFile(modelFile, "utf-8")
		const data = JSON.parse(content) as ModelState

		return {
			recent: Array.isArray(data.recent) ? data.recent : [],
			favorite: Array.isArray(data.favorite) ? data.favorite : [],
			variant: typeof data.variant === "object" && data.variant !== null ? data.variant : {},
		}
	} catch (err) {
		console.error("Failed to read model state:", err)
		return EMPTY_STATE
	}
}

// ============================================================
// Update recent model
// ============================================================

/**
 * Adds a model to the front of the recent list in model.json.
 * Deduplicates and caps at MAX_RECENT entries.
 * Matches the TUI's `model.set(model, { recent: true })` behavior.
 */
export async function updateModelRecent(model: ModelRef): Promise<ModelState> {
	try {
		const statePath = await resolveStatePath()
		const modelFile = join(statePath, "model.json")

		// Read existing state
		let existing: ModelState = { ...EMPTY_STATE }
		try {
			await access(modelFile)
			const content = await readFile(modelFile, "utf-8")
			const data = JSON.parse(content) as ModelState
			existing = {
				recent: Array.isArray(data.recent) ? data.recent : [],
				favorite: Array.isArray(data.favorite) ? data.favorite : [],
				variant: typeof data.variant === "object" && data.variant !== null ? data.variant : {},
			}
		} catch {
			// File doesn't exist or is invalid — start fresh
		}

		// Prepend model, deduplicate by providerID/modelID, cap at MAX_RECENT
		const key = (m: ModelRef) => `${m.providerID}/${m.modelID}`
		const seen = new Set<string>()
		const updated: ModelRef[] = []
		for (const entry of [model, ...existing.recent]) {
			const k = key(entry)
			if (!seen.has(k) && updated.length < MAX_RECENT) {
				seen.add(k)
				updated.push({ providerID: entry.providerID, modelID: entry.modelID })
			}
		}

		const newState: ModelState = {
			recent: updated,
			favorite: existing.favorite,
			variant: existing.variant,
		}

		// Ensure directory exists and write
		await mkdir(dirname(modelFile), { recursive: true })
		await writeFile(modelFile, JSON.stringify(newState), "utf-8")

		return newState
	} catch (err) {
		console.error("Failed to update model state:", err)
		return EMPTY_STATE
	}
}
