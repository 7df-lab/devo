import { describe, expect, test } from "bun:test"
import { toolPartFromUpdate } from "./acp-client-support"

describe("ACP tool update mapping", () => {
	test("infers command tools from raw input instead of exposing the tool call id", () => {
		const part = toolPartFromUpdate(
			"s1",
			{
				sessionUpdate: "tool_call_update",
				toolCallId: "call_00_4BNAahfLyysI8nCMiz3y9987",
				status: "completed",
				rawInput: {
					command: "pwd",
					description: "Print current directory",
				},
				rawOutput: "/repo",
			},
			undefined,
			1_772_000_000_000,
		)

		expect({
			callID: part.callID,
			tool: part.tool,
			title: part.state.title,
			output: part.state.output,
		}).toEqual({
			callID: "call_00_4BNAahfLyysI8nCMiz3y9987",
			tool: "bash",
			title: "bash",
			output: "/repo",
		})
	})
})
