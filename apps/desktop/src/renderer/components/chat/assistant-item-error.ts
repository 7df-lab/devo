import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"

/** Native assistant failures are carried by the envelope state or item error field. */
export function isAssistantItemError(info: NativeItemEnvelope): boolean {
	return (
		info.item.type === "assistantMessage" &&
		(info.state === "failed" || info.item.error != null)
	)
}
