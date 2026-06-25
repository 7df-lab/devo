import { describe, expect, test } from "bun:test"
import { convertMcpServers, convertSingleMcpServer, mergeMcpSources } from "../../src/converter/mcp"
import type { ClaudeMcpServer } from "../../src/types/claude-code"

describe("convertSingleMcpServer", () => {
	test("converts local stdio server", () => {
		const cc: ClaudeMcpServer = {
			command: "npx",
			args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
			env: { DEBUG: "true" },
		}
		const result = convertSingleMcpServer(cc)
		expect(result).toEqual({
			type: "local",
			command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
			environment: { DEBUG: "true" },
		})
	})

	test("converts local server without env", () => {
		const cc: ClaudeMcpServer = {
			command: "node",
			args: ["server.js"],
		}
		const result = convertSingleMcpServer(cc)
		expect(result).toEqual({
			type: "local",
			command: ["node", "server.js"],
		})
	})

	test("converts SSE remote server", () => {
		const cc: ClaudeMcpServer = {
			type: "sse",
			url: "https://mcp.example.com/sse",
		}
		const result = convertSingleMcpServer(cc)
		expect(result).toEqual({
			type: "remote",
			url: "https://mcp.example.com/sse",
		})
	})

	test("converts HTTP remote server", () => {
		const cc: ClaudeMcpServer = {
			type: "http",
			url: "https://mcp.example.com/api",
			headers: { Authorization: "Bearer token" },
		}
		const result = convertSingleMcpServer(cc)
		expect(result).toEqual({
			type: "remote",
			url: "https://mcp.example.com/api",
			headers: { Authorization: "Bearer token" },
		})
	})

	test("converts implicit remote (url without command)", () => {
		const cc: ClaudeMcpServer = {
			url: "https://mcp.example.com",
		}
		const result = convertSingleMcpServer(cc)
		expect(result).toEqual({
			type: "remote",
			url: "https://mcp.example.com",
		})
	})

	test("marks disabled servers", () => {
		const cc: ClaudeMcpServer = {
			command: "node",
			args: ["server.js"],
		}
		const result = convertSingleMcpServer(cc, true)
		expect(result).toEqual({
			type: "local",
			command: ["node", "server.js"],
			enabled: false,
		})
	})

	test("throws on remote server missing url", () => {
		const cc: ClaudeMcpServer = { type: "sse" }
		expect(() => convertSingleMcpServer(cc)).toThrow("missing url")
	})

	test("throws on local server missing command", () => {
		const cc: ClaudeMcpServer = {}
		expect(() => convertSingleMcpServer(cc)).toThrow("missing command")
	})
})

describe("convertMcpServers", () => {
	test("converts multiple servers with report", () => {
		const result = convertMcpServers({
			servers: {
				filesystem: {
					command: "npx",
					args: ["-y", "@modelcontextprotocol/server-filesystem"],
				},
				remote: {
					type: "sse",
					url: "https://mcp.example.com/sse",
				},
			},
		})

		expect(Object.keys(result.mcp)).toEqual(["filesystem", "remote"])
		expect(result.report.migrated.length).toBe(2)
		expect(result.report.errors.length).toBe(0)
	})

	test("handles disabled servers", () => {
		const result = convertMcpServers({
			servers: {
				myserver: { command: "node", args: ["server.js"] },
			},
			disabledServers: ["myserver"],
		})

		expect(result.mcp.myserver).toMatchObject({ enabled: false })
	})

	test("reports errors for invalid servers", () => {
		const result = convertMcpServers({
			servers: {
				broken: { type: "sse" }, // missing url
			},
		})

		expect(Object.keys(result.mcp)).toEqual([])
		expect(result.report.errors.length).toBe(1)
	})

	test("warns about embedded credentials in URLs", () => {
		const result = convertMcpServers({
			servers: {
				remote: { type: "sse", url: "https://example.com?token=secret123" },
			},
		})

		expect(result.report.warnings.length).toBeGreaterThan(0)
		expect(result.report.warnings[0]).toContain("embedded credentials")
	})

	test("warns about secrets in environment", () => {
		const result = convertMcpServers({
			servers: {
				local: {
					command: "node",
					args: ["server.js"],
					env: { API_KEY: "sk-really-long-secret-key-value" },
				},
			},
		})

		expect(result.report.warnings.length).toBeGreaterThan(0)
		expect(result.report.warnings[0]).toContain("secret")
	})
})

describe("mergeMcpSources", () => {
	test("merges from multiple sources", () => {
		const merged = mergeMcpSources(
			{
				servers: { a: { command: "node", args: ["a.js"] } },
				sourceDescription: "source1",
			},
			{
				servers: { b: { command: "node", args: ["b.js"] } },
				sourceDescription: "source2",
			},
		)

		expect(Object.keys(merged.servers)).toEqual(["a", "b"])
	})

	test("later sources override earlier ones", () => {
		const merged = mergeMcpSources(
			{
				servers: { a: { command: "node", args: ["old.js"] } },
			},
			{
				servers: { a: { command: "node", args: ["new.js"] } },
			},
		)

		expect(merged.servers.a.args).toEqual(["new.js"])
	})

	test("merges disabled server lists", () => {
		const merged = mergeMcpSources(
			{ servers: {}, disabledServers: ["a"] },
			{ servers: {}, disabledServers: ["b"] },
		)

		expect(merged.disabledServers).toEqual(["a", "b"])
	})
})
