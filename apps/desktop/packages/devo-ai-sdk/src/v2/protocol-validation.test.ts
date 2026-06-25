import { describe, expect, test } from "bun:test"
import {
	ProtocolValidationError,
	assertValidProtocolPayload,
} from "./protocol-validation"

describe("desktop protocol runtime validation", () => {
	test("accepts valid ACP session update notifications", () => {
		const payload = {
			sessionId: "s1",
			update: {
				sessionUpdate: "agent_message_chunk",
				content: { type: "text", text: "hello" },
			},
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "session/update",
				payload,
			}),
		).toBe(payload)
	})

	test("rejects malformed ACP session update notifications", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "session/update",
				payload: {
					update: {
						sessionUpdate: "agent_message_chunk",
						content: { type: "text", text: "hello" },
					},
				},
			}),
		).toThrow(ProtocolValidationError)
	})

	test("rejects malformed outgoing ACP prompt params", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "session/prompt",
				payload: {
					prompt: [{ type: "text", text: "missing session id" }],
				},
			}),
		).toThrow(/session\/prompt/)
	})

	test("validates incoming ACP results", () => {
		const payload = {
			sessions: [{ sessionId: "s1", cwd: "/repo" }],
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "session/list",
				payload,
			}),
		).toBe(payload)
		expect(() =>
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "session/list",
				payload: { sessions: [{ cwd: "/repo" }] },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates incoming ACP permission requests and outgoing responses", () => {
		const requestPayload = {
			sessionId: "s1",
			toolCall: { toolCallId: "tool1", title: "Run tests" },
			options: [{ optionId: "allow-once", name: "Allow once", kind: "allow_once" }],
		}
		const responsePayload = {
			outcome: { outcome: "selected", optionId: "allow-once" },
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingRequest",
				method: "session/request_permission",
				payload: requestPayload,
			}),
		).toBe(requestPayload)
		expect(
			assertValidProtocolPayload({
				direction: "outgoingResponse",
				method: "session/request_permission",
				payload: responsePayload,
			}),
		).toBe(responsePayload)
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingResponse",
				method: "session/request_permission",
				payload: { outcome: { outcome: "selected" } },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates non-ACP goal request params from generated Rust schema", () => {
		const payload = { sessionId: "s1" }

		expect(
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "goal/status",
				payload,
			}),
		).toBe(payload)
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "goal/status",
				payload: {},
			}),
		).toThrow(ProtocolValidationError)
	})

	test("rejects unknown protocol methods", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "unknown/method",
				payload: {},
			}),
		).toThrow(/unknown protocol method/)
	})
})
