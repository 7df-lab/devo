/**
 * Validator module.
 *
 * Validates conversion output against the Devo config schema.
 * Uses structural checks rather than importing Zod schemas from Devo
 * (which are not published as a separate package).
 */
import type { ConversionResult } from "../types/conversion-result"
import type { DevoConfig } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

export interface ValidationResult {
	valid: boolean
	errors: ValidationError[]
	warnings: string[]
	report: MigrationReport
}

export interface ValidationError {
	path: string
	message: string
	value?: unknown
}

/**
 * Validate a conversion result against the Devo schema.
 *
 * @param conversion - Output from `convert()`
 * @returns Validation result with errors and warnings
 */
export function validate(conversion: ConversionResult): ValidationResult {
	const errors: ValidationError[] = []
	const warnings: string[] = []

	// Validate global config
	validateConfig(conversion.globalConfig, "globalConfig", errors, warnings)

	// Validate per-project configs
	for (const [projectPath, config] of conversion.projectConfigs) {
		validateConfig(config, `projectConfigs["${projectPath}"]`, errors, warnings)
	}

	// Validate agent files have frontmatter
	for (const [path, content] of conversion.agents) {
		if (!content.startsWith("---")) {
			errors.push({
				path: `agents["${path}"]`,
				message: "Agent file missing YAML frontmatter",
			})
		}
	}

	// Validate command files have frontmatter
	for (const [path, content] of conversion.commands) {
		if (!content.startsWith("---")) {
			errors.push({
				path: `commands["${path}"]`,
				message: "Command file missing YAML frontmatter",
			})
		}
	}

	const report = createEmptyReport()
	if (errors.length > 0) {
		report.errors.push(...errors.map((e) => `Validation error at ${e.path}: ${e.message}`))
	}
	report.warnings.push(...warnings)

	return {
		valid: errors.length === 0,
		errors,
		warnings,
		report,
	}
}

/**
 * Validate a single Devo config object.
 */
function validateConfig(
	config: Partial<DevoConfig>,
	prefix: string,
	errors: ValidationError[],
	warnings: string[],
): void {
	// Model format: must be "provider/model"
	if (config.model !== undefined) {
		if (!config.model.includes("/")) {
			errors.push({
				path: `${prefix}.model`,
				message: `Model ID must be in "provider/model" format, got "${config.model}"`,
				value: config.model,
			})
		}
	}

	if (config.small_model !== undefined) {
		if (!config.small_model.includes("/")) {
			errors.push({
				path: `${prefix}.small_model`,
				message: `Small model ID must be in "provider/model" format, got "${config.small_model}"`,
				value: config.small_model,
			})
		}
	}

	// MCP servers
	if (config.mcp) {
		for (const [name, mcp] of Object.entries(config.mcp)) {
			validateMcp(name, mcp, `${prefix}.mcp["${name}"]`, errors, warnings)
		}
	}

	// Agents
	if (config.agent) {
		for (const [name, agent] of Object.entries(config.agent)) {
			if (agent) {
				validateAgent(name, agent, `${prefix}.agent["${name}"]`, errors, warnings)
			}
		}
	}

	// Permission
	if (config.permission) {
		validatePermission(config.permission, `${prefix}.permission`, errors, warnings)
	}

	// Provider
	if (config.provider) {
		for (const [name, provider] of Object.entries(config.provider)) {
			if (provider?.options) {
				// Check for accidentally embedded secrets
				for (const key of Object.keys(provider.options)) {
					const value = provider.options[key]
					if (
						typeof value === "string" &&
						/key|token|secret|password/i.test(key) &&
						!value.startsWith("{env:")
					) {
						warnings.push(
							`${prefix}.provider["${name}"].options.${key} may contain a hardcoded secret. ` +
								`Consider using {env:VAR} interpolation.`,
						)
					}
				}
			}
		}
	}

	// Share
	if (config.share !== undefined) {
		const validShare = ["manual", "auto", "disabled"]
		if (!validShare.includes(config.share)) {
			errors.push({
				path: `${prefix}.share`,
				message: `Invalid share value: "${config.share}". Must be one of: ${validShare.join(", ")}`,
				value: config.share,
			})
		}
	}

	// Autoupdate
	if (config.autoupdate !== undefined) {
		if (typeof config.autoupdate !== "boolean" && config.autoupdate !== "notify") {
			errors.push({
				path: `${prefix}.autoupdate`,
				message: `Invalid autoupdate value. Must be boolean or "notify"`,
				value: config.autoupdate,
			})
		}
	}
}

function validateMcp(
	_name: string,
	mcp: unknown,
	prefix: string,
	errors: ValidationError[],
	warnings: string[],
): void {
	if (typeof mcp !== "object" || mcp === null) {
		errors.push({ path: prefix, message: "MCP config must be an object" })
		return
	}

	const obj = mcp as Record<string, unknown>

	// Simple enable/disable toggle
	if ("enabled" in obj && Object.keys(obj).length === 1) {
		if (typeof obj.enabled !== "boolean") {
			errors.push({ path: `${prefix}.enabled`, message: "enabled must be boolean" })
		}
		return
	}

	if (!("type" in obj)) {
		errors.push({
			path: prefix,
			message: 'MCP config must have a "type" field ("local" or "remote")',
		})
		return
	}

	if (obj.type === "local") {
		if (!Array.isArray(obj.command) || obj.command.length === 0) {
			errors.push({
				path: `${prefix}.command`,
				message: "Local MCP server must have a non-empty command array",
			})
		}
		if (obj.environment !== undefined && typeof obj.environment !== "object") {
			errors.push({
				path: `${prefix}.environment`,
				message: "environment must be an object",
			})
		}
	} else if (obj.type === "remote") {
		if (typeof obj.url !== "string" || !obj.url) {
			errors.push({
				path: `${prefix}.url`,
				message: "Remote MCP server must have a url string",
			})
		}
		// Check for embedded credentials in URL
		if (typeof obj.url === "string" && /[?&](token|key|secret|api_key)=/i.test(obj.url)) {
			warnings.push(
				`${prefix}.url contains embedded credentials. Consider using headers or OAuth instead.`,
			)
		}
	} else {
		errors.push({
			path: `${prefix}.type`,
			message: `Invalid MCP type: "${obj.type}". Must be "local" or "remote"`,
			value: obj.type,
		})
	}
}

function validateAgent(
	_name: string,
	agent: Record<string, unknown>,
	prefix: string,
	errors: ValidationError[],
	_warnings: string[],
): void {
	if (agent.mode !== undefined) {
		const validModes = ["subagent", "primary", "all"]
		if (!validModes.includes(agent.mode as string)) {
			errors.push({
				path: `${prefix}.mode`,
				message: `Invalid agent mode: "${agent.mode}". Must be one of: ${validModes.join(", ")}`,
				value: agent.mode,
			})
		}
	}

	if (agent.temperature !== undefined) {
		const temp = agent.temperature as number
		if (typeof temp !== "number" || temp < 0 || temp > 2) {
			errors.push({
				path: `${prefix}.temperature`,
				message: `Temperature must be a number between 0 and 2`,
				value: temp,
			})
		}
	}

	if (agent.steps !== undefined) {
		const steps = agent.steps as number
		if (typeof steps !== "number" || !Number.isInteger(steps) || steps < 1) {
			errors.push({
				path: `${prefix}.steps`,
				message: `Steps must be a positive integer`,
				value: steps,
			})
		}
	}

	if (agent.model !== undefined && typeof agent.model === "string") {
		if (!agent.model.includes("/")) {
			errors.push({
				path: `${prefix}.model`,
				message: `Agent model must be in "provider/model" format, got "${agent.model}"`,
				value: agent.model,
			})
		}
	}
}

function validatePermission(
	permission: unknown,
	prefix: string,
	errors: ValidationError[],
	_warnings: string[],
): void {
	// Permission can be a string action or an object
	if (typeof permission === "string") {
		const validActions = ["allow", "deny", "ask"]
		if (!validActions.includes(permission)) {
			errors.push({
				path: prefix,
				message: `Invalid permission action: "${permission}". Must be one of: ${validActions.join(", ")}`,
				value: permission,
			})
		}
		return
	}

	if (typeof permission !== "object" || permission === null) {
		errors.push({ path: prefix, message: "Permission must be a string action or an object" })
		return
	}

	const validActions = ["allow", "deny", "ask"]
	for (const [key, value] of Object.entries(permission as Record<string, unknown>)) {
		if (key === "__originalKeys") continue

		if (typeof value === "string") {
			if (!validActions.includes(value)) {
				errors.push({
					path: `${prefix}["${key}"]`,
					message: `Invalid permission action: "${value}"`,
					value,
				})
			}
		} else if (typeof value === "object" && value !== null) {
			for (const [pattern, action] of Object.entries(value as Record<string, unknown>)) {
				if (typeof action !== "string" || !validActions.includes(action)) {
					errors.push({
						path: `${prefix}["${key}"]["${pattern}"]`,
						message: `Invalid permission action: "${action}"`,
						value: action,
					})
				}
			}
		}
	}
}
