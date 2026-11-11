import assert from "node:assert/strict";
import { test } from "node:test";
import type { DaemonTransportClient } from "./daemon-client.js";
import { DAEMON_COMMAND_COMPATIBILITY, DAEMON_COMMAND_PLANE, DAEMON_OUTBOUND_COMPATIBILITY } from "./daemon-protocol.js";
import { listDaemonScheduledTasks } from "./scheduled-catalog.js";

test("Scheduled catalog preserves legacy daemon command and payload keys", async () => {
	const commands: unknown[] = [];
	const jobs = [{ job: { id: "scheduled-1" } }];
	const client = {
		hello: {},
		supportsServerCapability: (capability: string) => capability === "heartbeat_catalog",
		request: async (command: unknown) => {
			commands.push(command);
			return { success: true, data: { heartbeats: jobs } };
		},
	} as unknown as DaemonTransportClient;
	assert.deepEqual(await listDaemonScheduledTasks(client, "session-1"), jobs);
	assert.deepEqual(commands, [{ type: "heartbeats_list", activeSessionId: "session-1" }]);
	assert.deepEqual(DAEMON_COMMAND_COMPATIBILITY.heartbeats_list, { minProtocol: 7, capability: "heartbeat_catalog" });
	assert.equal(DAEMON_COMMAND_PLANE.heartbeat_manage, "control");
	assert.deepEqual(DAEMON_OUTBOUND_COMPATIBILITY.heartbeats_changed, { minProtocol: 7, capability: "heartbeat_catalog" });
});
