/** Presentation-only metadata; Native remains the source of truth. */
export type { HostMessageOrigin } from "@devo-ai/sdk/v2/client"

export function hostMessageLabel(origin: unknown): string {
	const metadata =
		origin && typeof origin === "object"
			? (origin as Record<string, unknown>)
			: {}
	const labels: Record<string, string> = {
		agent: "Agent message",
		backgroundTask: "Background notification",
		schedule: "Scheduled notification",
	}
	return `${labels[String(metadata.source)] ?? "Host notification"} · ${String(metadata.sender ?? "Host")}`
}
