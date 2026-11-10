import { describe, expect, test } from "bun:test";
import { createDevoClient, type DevoNativeTransport } from "./client";
import {
	assertValidProtocolPayload,
	ProtocolValidationError,
} from "./protocol-validation";

describe("Native MCP reconnect", () => {
	test("uses the native method with only a name and preserves the server result", async () => {
		const requests: unknown[] = [];
		const result = {
			servers: [{ name: "docs", status: "connected", toolCount: 1 }],
		};
		const transport: DevoNativeTransport = {
			async request(method, params, directory) {
				requests.push({ method, params, directory });
				if (method === "initialize")
					return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] };
				return result;
			},
			async respond() {},
			subscribe() {
				return () => {};
			},
			connected() {
				return true;
			},
		};
		const client = createDevoClient({ directory: "/repo", transport });
		const params = { name: "docs", password: "must-not-be-forwarded" };
		expect(await client.mcp.reconnect(params)).toEqual({ data: result });
		expect(requests.at(-1)).toEqual({
			method: "mcp/reconnect",
			params: { name: "docs" },
			directory: "/repo",
		});
	});

	test("validates name-only requests and rejects credential or enablement fields", () => {
		const payload = { name: "docs" };
		expect(
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "mcp/reconnect",
				payload,
			}),
		).toEqual(payload);
		for (const invalid of [
			{},
			{ name: 42 },
			{ name: "docs", enabled: true },
			{ name: "docs", credentials: {} },
			{ name: "docs", token: "test-token" },
		]) {
			expect(() =>
				assertValidProtocolPayload({
					direction: "outgoingRequest",
					method: "mcp/reconnect",
					payload: invalid,
				}),
			).toThrow(ProtocolValidationError);
		}
	});

	test("validates reconnect results", () => {
		const payload = {
			servers: [{ name: "docs", status: "failed", toolCount: 0 }],
		};
		expect(
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "mcp/reconnect",
				payload,
			}),
		).toEqual(payload);
		expect(() =>
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "mcp/reconnect",
				payload: { servers: [{ name: "docs", toolCount: "many" }] },
			}),
		).toThrow(ProtocolValidationError);
	});
});
