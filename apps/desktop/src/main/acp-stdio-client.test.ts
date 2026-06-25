import { describe, expect, test } from "bun:test"
import { StdioAcpClient, buildServerProcessEnv, routeAcpLine } from "./acp-stdio-client"

describe("routeAcpLine", () => {
	test("routes JSON-RPC responses, notifications, and server requests", () => {
		const response = routeAcpLine(JSON.stringify({ jsonrpc: "2.0", id: 7, result: { ok: true } }))
		expect(response).toEqual({
			type: "response",
			id: 7,
			message: { jsonrpc: "2.0", id: 7, result: { ok: true } },
		})

		const notification = routeAcpLine(
			JSON.stringify({
				jsonrpc: "2.0",
				method: "session/update",
				params: { sessionId: "s1", update: { sessionUpdate: "agent_message_chunk" } },
			}),
		)
		expect(notification).toEqual({
			type: "notification",
			method: "session/update",
			params: { sessionId: "s1", update: { sessionUpdate: "agent_message_chunk" } },
		})

		const request = routeAcpLine(
			JSON.stringify({
				jsonrpc: "2.0",
				id: 9,
				method: "session/request_permission",
				params: { sessionId: "s1", options: [{ optionId: "reject", kind: "reject_once" }] },
			}),
		)
		expect(request).toEqual({
			type: "request",
			id: 9,
			method: "session/request_permission",
			params: { sessionId: "s1", options: [{ optionId: "reject", kind: "reject_once" }] },
		})
	})
})

describe("StdioAcpClient", () => {
	test("builds server env with bin dir first while preserving caller env", () => {
		const env = buildServerProcessEnv({
			baseEnv: { PATH: "/usr/bin", KEEP: "base" },
			homeDir: "/Users/tester",
			optionsEnv: { DEVO_SUPPRESS_SERVER_TRAY: "1", PATH: "/custom/bin" },
			pathSeparator: ":",
		})

		expect(env).toMatchObject({
			DEVO_SUPPRESS_SERVER_TRAY: "1",
			KEEP: "base",
			PATH: "/Users/tester/.devo/bin:/custom/bin",
		})
	})

	test("rejects and clears pending requests when stdin write fails", async () => {
		const client = new StdioAcpClient()
		const epipe = Object.assign(new Error("write EPIPE"), { code: "EPIPE" })
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: () => {
					throw epipe
				},
			},
		}

		await expect(client.request("initialize")).rejects.toThrow("write EPIPE")
		expect((client as unknown as { pending: Map<unknown, unknown> }).pending.size).toBe(0)
		expect(client.connected()).toBe(false)
	})
})
