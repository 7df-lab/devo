import type { AgentConnectionScheduledTask } from "../agent-connection/types.js";
import type { DaemonTransportClient } from "./daemon-client.js";
import { deserializeDaemonError } from "./daemon-errors.js";
import { isUnknownDaemonCommandError } from "./daemon-protocol.js";

export async function listDaemonScheduledTasks(
	client: DaemonTransportClient,
	activeSessionId?: string,
): Promise<AgentConnectionScheduledTask[]> {
	if (!client.hello) await client.waitForHello();
	if (!client.supportsServerCapability("heartbeat_catalog")) return [];
	try {
		const command = { type: "heartbeats_list", ...(activeSessionId ? { activeSessionId } : {}) } as const;
		const response = await client.request(command);
		if (!response.success) {
			throw deserializeDaemonError(response);
		}
		// Legacy daemon payload key is a wire contract, not the TUI display name.
		return (response.data as { heartbeats: AgentConnectionScheduledTask[] }).heartbeats;
	} catch (error) {
		if (isUnknownDaemonCommandError(error, "heartbeats_list")) {
			return [];
		}
		throw error;
	}
}
