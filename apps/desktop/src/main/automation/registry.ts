/**
 * Automation config file registry.
 *
 * Manages automation definitions stored as JSON + prompt.md files
 * on disk at $DEVO_HOME/automations/<id>/. Provides CRUD operations
 * with atomic writes and validation.
 */

import crypto from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import { createLogger } from "../logger"
import { getConfigDir } from "./paths"
import type { AutomationConfig, CreateAutomationInput, UpdateAutomationInput } from "./types"

const log = createLogger("automation-registry")

const AUTOMATIONS_DIR = "automations"
const CONFIG_FILE = "config.json"
const PROMPT_FILE = "prompt.md"

// ============================================================
// Helpers
// ============================================================

function getAutomationsDir(): string {
	return path.join(getConfigDir(), AUTOMATIONS_DIR)
}

function getAutomationDir(id: string): string {
	return path.join(getAutomationsDir(), id)
}

/** Generate a URL-safe slug from a name, with dedup suffix if needed. */
function slugify(name: string): string {
	return name
		.toLowerCase()
		.trim()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/-+/g, "-")
		.replace(/^-+/, "")
		.replace(/-+$/, "")
}

function generateId(name: string): string {
	const base = slugify(name) || "automation"
	const dir = getAutomationsDir()
	const existing = new Set(fs.existsSync(dir) ? fs.readdirSync(dir) : [])

	if (!existing.has(base)) return base
	for (let i = 2; i <= 20; i++) {
		const candidate = `${base}-${i}`
		if (!existing.has(candidate)) return candidate
	}
	return `${base}-${crypto.randomUUID().slice(0, 8)}`
}

function isValidId(id: string): boolean {
	return !(!id || id === "." || id === ".." || id.includes("/") || id.includes("\\"))
}

/** Atomic file write: write to temp file, then rename. */
function atomicWrite(filePath: string, content: string): void {
	const dir = path.dirname(filePath)
	fs.mkdirSync(dir, { recursive: true })
	const tmpPath = path.join(dir, `.${path.basename(filePath)}.tmp-${Date.now()}`)
	fs.writeFileSync(tmpPath, content, "utf-8")
	try {
		fs.renameSync(tmpPath, filePath)
	} catch {
		try {
			fs.unlinkSync(filePath)
		} catch {
			/* ignore */
		}
		fs.renameSync(tmpPath, filePath)
	}
}

// ============================================================
// Registry API
// ============================================================

/** Read a single automation config from disk. Returns null if not found or invalid. */
export function readConfig(id: string): (AutomationConfig & { id: string; prompt: string }) | null {
	if (!isValidId(id)) return null

	const dir = getAutomationDir(id)
	const configPath = path.join(dir, CONFIG_FILE)
	const promptPath = path.join(dir, PROMPT_FILE)

	try {
		const raw = JSON.parse(fs.readFileSync(configPath, "utf-8"))
		const prompt = fs.existsSync(promptPath) ? fs.readFileSync(promptPath, "utf-8") : ""
		return { ...raw, id, prompt }
	} catch (err) {
		if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
			log.warn(`Failed to read automation config: ${id}`, err)
		}
		return null
	}
}

/** List all valid automation configs from disk. */
export function listConfigs(): (AutomationConfig & { id: string; prompt: string })[] {
	const dir = getAutomationsDir()
	if (!fs.existsSync(dir)) return []

	const results: (AutomationConfig & { id: string; prompt: string })[] = []
	for (const entry of fs.readdirSync(dir)) {
		if (!isValidId(entry)) continue
		const config = readConfig(entry)
		if (config && config.status !== "archived") {
			results.push(config)
		}
	}
	return results
}

/** Create a new automation. Returns the generated ID. */
export function createConfig(input: CreateAutomationInput): string {
	const id = generateId(input.name)
	const dir = getAutomationDir(id)

	const config: Omit<AutomationConfig, "prompt"> = {
		version: 1,
		name: input.name,
		status: "active",
		schedule: {
			rrule: input.schedule.rrule,
			timezone: input.schedule.timezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone,
		},
		workspaces: input.workspaces,
		execution: {
			model: input.execution?.model,
			effort: input.execution?.effort ?? "medium",
			timeout: input.execution?.timeout ?? 600,
			retries: input.execution?.retries ?? 0,
			retryDelay: input.execution?.retryDelay ?? 60,
			parallelWorkspaces: input.execution?.parallelWorkspaces ?? false,
			approvalPolicy: input.execution?.approvalPolicy ?? "never",
			useWorktree: input.execution?.useWorktree ?? true,
			permissionPreset: input.execution?.permissionPreset ?? "default",
		},
	}

	atomicWrite(path.join(dir, CONFIG_FILE), JSON.stringify(config, null, "\t"))
	atomicWrite(path.join(dir, PROMPT_FILE), input.prompt)

	log.info("Created automation", { id, name: input.name })
	return id
}

/** Update an existing automation config. */
export function updateConfig(input: UpdateAutomationInput): boolean {
	const existing = readConfig(input.id)
	if (!existing) return false

	const { id: _id, prompt: _existingPrompt, ...existingConfig } = existing

	const updated = { ...existingConfig }
	if (input.name !== undefined) updated.name = input.name
	if (input.status !== undefined) updated.status = input.status
	if (input.schedule !== undefined) {
		updated.schedule = {
			rrule: input.schedule.rrule,
			timezone: input.schedule.timezone ?? existingConfig.schedule.timezone,
		}
	}
	if (input.workspaces !== undefined) updated.workspaces = input.workspaces
	if (input.execution !== undefined) {
		updated.execution = { ...existingConfig.execution, ...input.execution }
	}

	const dir = getAutomationDir(input.id)
	atomicWrite(path.join(dir, CONFIG_FILE), JSON.stringify(updated, null, "\t"))

	if (input.prompt !== undefined) {
		atomicWrite(path.join(dir, PROMPT_FILE), input.prompt)
	}

	log.info("Updated automation", { id: input.id })
	return true
}

/** Delete an automation directory from disk. */
export function deleteConfig(id: string): boolean {
	if (!isValidId(id)) return false
	const dir = getAutomationDir(id)
	try {
		fs.rmSync(dir, { recursive: true, force: true })
		log.info("Deleted automation config from disk", { id })
		return true
	} catch (err) {
		log.error("Failed to delete automation config", { id }, err)
		return false
	}
}
