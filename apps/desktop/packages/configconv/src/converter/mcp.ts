/**
 * MCP server configuration converter.
 *
 * Claude Code: { mcpServers: { name: { command, args, env, type, url } } }
 * Devo:    { mcp: { name: { type: "local"|"remote", command[], environment } } }
 */
import type { ClaudeMcpServer } from "../types/claude-code"
import type { DevoMcp, DevoMcpLocal, DevoMcpRemote } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

export interface McpConversionInput {
	/** MCP servers from all sources, keyed by server name */
	servers: Record<string, ClaudeMcpServer>
	/** Server names that should be disabled */
	disabledServers?: string[]
	/** Source description for reporting */
	sourceDescription?: string
}

export interface McpConversionResult {
	mcp: Record<string, DevoMcp>
	report: MigrationReport
}

/**
 * Convert Claude Code MCP server configs to Devo format.
 */
export function convertMcpServers(input: McpConversionInput): McpConversionResult {
	const mcp: Record<string, DevoMcp> = {}
	const report = createEmptyReport()
	const source = input.sourceDescription ?? "Claude Code"
	const disabledSet = new Set(input.disabledServers ?? [])

	for (const [name, server] of Object.entries(input.servers)) {
		const isDisabled = disabledSet.has(name)

		try {
			const converted = convertSingleMcpServer(server, isDisabled)
			mcp[name] = converted

			const targetType = "type" in converted ? converted.type : "unknown"
			report.migrated.push({
				category: "mcp",
				source: `${source}: ${name}`,
				target: `mcp.${name}`,
				details: `${detectSourceType(server)} -> ${targetType}${isDisabled ? " (disabled)" : ""}`,
			})

			// Warn about embedded tokens
			if ("url" in converted && typeof converted.url === "string") {
				if (/[?&](token|key|secret|api_key)=/i.test(converted.url)) {
					report.warnings.push(
						`MCP server "${name}": URL contains embedded credentials. ` +
							`Consider using {env:TOKEN_VAR} interpolation in Devo.`,
					)
				}
			}
			if ("environment" in converted && converted.environment) {
				for (const [envKey, envVal] of Object.entries(converted.environment)) {
					if (/key|token|secret|password/i.test(envKey) && envVal.length > 8) {
						report.warnings.push(
							`MCP server "${name}": environment variable "${envKey}" may contain a secret. ` +
								`Consider using {env:${envKey}} interpolation.`,
						)
					}
				}
			}
		} catch (err) {
			report.errors.push(
				`Failed to convert MCP server "${name}": ${err instanceof Error ? err.message : String(err)}`,
			)
		}
	}

	return { mcp, report }
}

/**
 * Convert a single MCP server config.
 */
export function convertSingleMcpServer(
	server: ClaudeMcpServer,
	disabled: boolean = false,
): DevoMcpLocal | DevoMcpRemote {
	// Determine if this is a remote or local server
	if (isRemoteServer(server)) {
		return convertRemoteMcp(server, disabled)
	}
	return convertLocalMcp(server, disabled)
}

function isRemoteServer(server: ClaudeMcpServer): boolean {
	// Explicit remote types
	if (server.type === "sse" || server.type === "http") return true
	// Has URL but no command
	if (server.url && !server.command) return true
	return false
}

function convertRemoteMcp(server: ClaudeMcpServer, disabled: boolean): DevoMcpRemote {
	if (!server.url) {
		throw new Error("Remote MCP server missing url")
	}

	const result: DevoMcpRemote = {
		type: "remote",
		url: server.url,
	}

	if (disabled) result.enabled = false
	if (server.headers) result.headers = { ...server.headers }

	return result
}

function convertLocalMcp(server: ClaudeMcpServer, disabled: boolean): DevoMcpLocal {
	if (!server.command) {
		throw new Error("Local MCP server missing command")
	}

	// Merge command + args into a single array
	const command: string[] = Array.isArray(server.command)
		? [...(server.command as unknown as string[])]
		: [server.command, ...(server.args ?? [])]

	const result: DevoMcpLocal = {
		type: "local",
		command,
	}

	if (server.env && Object.keys(server.env).length > 0) {
		result.environment = { ...server.env }
	}

	if (disabled) result.enabled = false

	return result
}

function detectSourceType(server: ClaudeMcpServer): string {
	if (server.type) return server.type
	if (server.url) return "remote (implicit)"
	if (server.command) return "local (implicit)"
	return "unknown"
}

/**
 * Merge MCP configs from multiple sources, handling deduplication.
 * Later sources override earlier ones for the same server name.
 */
export function mergeMcpSources(...sources: McpConversionInput[]): McpConversionInput {
	const merged: Record<string, ClaudeMcpServer> = {}
	const disabledServers = new Set<string>()

	for (const source of sources) {
		for (const [name, server] of Object.entries(source.servers)) {
			merged[name] = server
		}
		if (source.disabledServers) {
			for (const name of source.disabledServers) {
				disabledServers.add(name)
			}
		}
	}

	return {
		servers: merged,
		disabledServers: [...disabledServers],
		sourceDescription: "merged sources",
	}
}
