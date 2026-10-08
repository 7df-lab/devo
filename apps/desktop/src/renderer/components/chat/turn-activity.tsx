import { CodeBlock, CodeBlockContent, CodeBlockCopyButton } from "@devo/ui/components/ai-elements/code-block"
import { MessageResponse } from "@devo/ui/components/ai-elements/message"
import { nativeItemType } from "@devo-ai/sdk/v2/client"
import { memo, useMemo } from "react"
import { toast } from "sonner"
import type { ChatMessageEntry } from "../../hooks/use-session-chat"
import { itemDisplayText } from "../../atoms/derived/session-chat"
import { formatNativeToolDetails, getNativePythonToolCode } from "../../lib/tool-details"
import { TranscriptDisclosure, TranscriptDisclosureContent, TranscriptDisclosureTrigger } from "./transcript-disclosure"
import { activityRowStatus, activityRowTitle, activitySummary, buildActivityRows, type ActivityRow } from "./turn-activity-model"

function ActivityDetails({ row, working }: { row: ActivityRow; working: boolean }) {
	const type = nativeItemType(row.entry.info)
	const title = activityRowTitle(row)
	const status = activityRowStatus(row)
	const code = getNativePythonToolCode(row.entry.info.item)
	const output = row.result ? formatNativeToolDetails(row.result.info.item) : undefined

	if (type === "reasoning") {
		return (
			<section className="devo-activity-thought" aria-label="Thinking details">
				<MessageResponse streaming={working && status === "Running"} className="devo-reasoning-response">
					{itemDisplayText(row.entry.info)}
				</MessageResponse>
			</section>
		)
	}

	return (
		<section className="devo-activity-tool" aria-label={`${title} details`}>
			{code !== undefined ? (
				<CodeBlock code={code} language="python" className="devo-activity-code">
					<div className="devo-activity-tool-heading">
						<CodeBlockCopyButton
							className="devo-activity-copy"
							aria-label="Copy Python code"
							title="Copy code"
							onCopy={() => toast.success("Python code copied")}
							onError={() => toast.error("Could not copy Python code")}
						>Copy</CodeBlockCopyButton>
					</div>
					<CodeBlockContent code={code} language="python" />
				</CodeBlock>
			) : (
				<pre className="devo-activity-output">{formatNativeToolDetails(row.entry.info.item)}</pre>
			)}
			{output !== undefined && (
				<div className="devo-activity-result">
					<div className="devo-activity-caption">Output</div>
					<pre className="devo-activity-output">{output.trim() || "No output"}</pre>
				</div>
			)}
		</section>
	)
}

/** Each thought or tool owns its disclosure; tool source and output stay together. */
export const TurnActivity = memo(function TurnActivity({ entries, working }: {
	entries: ChatMessageEntry[]
	working: boolean
}) {
	const rows = useMemo(() => buildActivityRows(entries), [entries])
	const summary = activitySummary(entries, working)
	const failed = rows.some((row) => ["Failed", "Connection lost"].includes(activityRowStatus(row) ?? ""))
	return (
		<TranscriptDisclosure key={working ? "working" : "completed"} className="devo-turn-activity" expandable={rows.length > 0}>
			<TranscriptDisclosureTrigger
				className={`devo-activity-summary${working ? " devo-activity-summary--working" : ""}${failed ? " text-destructive" : ""}`}
				label={<span aria-live="polite" aria-atomic="true">{summary}</span>}
				aria-label={`${summary}: activity details`}
			/>
			{rows.length > 0 && (
				<TranscriptDisclosureContent className="devo-activity-rows">
					{rows.map((row) => {
						const title = activityRowTitle(row)
						const status = activityRowStatus(row)
						const rowFailed = status === "Failed" || status === "Connection lost"
						return (
							<TranscriptDisclosure key={row.entry.info.id} className="devo-activity-row">
								<TranscriptDisclosureTrigger
									className={`devo-activity-summary${rowFailed ? " text-destructive" : ""}`}
									label={title}
									trailing={status && <span className="devo-activity-row-status">{status}</span>}
									aria-label={`${title}: activity details`}
								/>
								<TranscriptDisclosureContent className="devo-activity-details">
									<ActivityDetails row={row} working={working} />
								</TranscriptDisclosureContent>
							</TranscriptDisclosure>
						)
					})}
				</TranscriptDisclosureContent>
			)}
		</TranscriptDisclosure>
	)
})
