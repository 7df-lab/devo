import { hostMessageLabel } from "../../lib/host-message"

/** Host notifications must never expose the human prompt's edit/re-send controls. */
export function HostMessageBlock({
	text,
	origin,
}: {
	text: string
	origin: unknown
}) {
	const metadata =
		origin && typeof origin === "object"
			? (origin as Record<string, unknown>)
			: {}

	return (
		<div
			className="rounded border border-border/50 bg-muted/30 px-3 py-2 text-sm"
			data-host-message-source={String(metadata.source ?? "host")}
		>
			<div className="mb-1 text-xs font-medium text-muted-foreground">
				{hostMessageLabel(origin)}
			</div>
			<p className="whitespace-pre-wrap [overflow-wrap:break-word]">{text}</p>
		</div>
	)
}
