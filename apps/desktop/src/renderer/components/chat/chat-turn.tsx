/**
 * Native-first chat turn renderer.
 * Transcript rows use Native ItemEnvelope fields and shared disclosure styling.
 */
import {
	Message,
	MessageContent,
	MessageResponse,
} from "@devo/ui/components/ai-elements/message"
import {
	assistantOrReasoningText,
	isUserMessageItem,
	nativeItemType,
	userMessageText,
} from "@devo-ai/sdk/v2/client"
import { CopyIcon, SplitIcon } from "lucide-react"
import { memo, useCallback, useMemo, useState } from "react"
import type { ChatMessageEntry, ChatTurn as ChatTurnType } from "../../hooks/use-session-chat"
import type { ProviderErrorEntry, ProviderRetryStatus } from "../../atoms/sessions"
import type { Agent } from "../../lib/types"
import { itemDisplayText } from "../../atoms/derived/session-chat"
import { UserMessageBlock } from "./user-message-block"
import { ProviderErrorRow } from "./provider-error-row"
import { TurnActivity } from "./turn-activity"
import { groupTurnSegments } from "./turn-activity-model"

export function isSyntheticMessage(entry: ChatMessageEntry): boolean {
	const type = nativeItemType(entry.info)
	return type === "contextCompaction" || type === "plan"
}

function getUserText(entry: ChatMessageEntry): string {
	return userMessageText(entry.info)
}

interface ChatTurnProps {
	turn: ChatTurnType
	isLast: boolean
	isWorking: boolean
	agent?: Agent | null
	isConnected?: boolean
	retryStatus?: ProviderRetryStatus
	providerErrors?: ProviderErrorEntry[]
	onForkFromTurn?: (turnId: string) => void
	onEditUserMessage?: (messageId: string, text: string) => void
}

function NativeItemRow({ entry, streaming }: { entry: ChatMessageEntry; streaming?: boolean }) {
	const type = nativeItemType(entry.info)
	const text = itemDisplayText(entry.info)
	const state = entry.info.state

	if (type === "contextCompaction") {
		return (
			<div className="text-center text-[11px] text-muted-foreground">
				{text || "Context compaction"} · {state}
			</div>
		)
	}

	if (type === "plan") {
		return (
			<div className="rounded border border-border/50 px-3 py-2 text-sm whitespace-pre-wrap">
				{text || "Plan"}
			</div>
		)
	}

	if (type === "warning") {
		return (
			<div role="status" className="border-l-2 border-border pl-3 text-sm text-muted-foreground whitespace-pre-wrap">
				{text}
			</div>
		)
	}

	if (type === "assistantMessage" || !type) {
		if (!text) return null
		return (
			<Message from="assistant">
				<MessageContent>
					<MessageResponse streaming={streaming}>{text}</MessageResponse>
				</MessageContent>
			</Message>
		)
	}

	if (!text) return null
	return (
		<div className="text-sm text-muted-foreground whitespace-pre-wrap">
			<span className="mr-2 text-[10px] uppercase tracking-wide opacity-60">{type}</span>
			{text}
		</div>
	)
}

function areTurnsEqual(a: ChatTurnType, b: ChatTurnType): boolean {
	if (a.id !== b.id || a.turnId !== b.turnId) return false
	if (a.userMessage.info.revision !== b.userMessage.info.revision) return false
	if (a.assistantMessages.length !== b.assistantMessages.length) return false
	for (let i = 0; i < a.assistantMessages.length; i++) {
		const left = a.assistantMessages[i].info
		const right = b.assistantMessages[i].info
		if (left.id !== right.id || left.revision !== right.revision || left.state !== right.state) {
			return false
		}
		if (assistantOrReasoningText(left).length !== assistantOrReasoningText(right).length) {
			return false
		}
	}
	return true
}

export const ChatTurnComponent = memo(
	function ChatTurnComponent({
		turn,
		isLast,
		isWorking,
		providerErrors = [],
		onForkFromTurn,
		onEditUserMessage,
	}: ChatTurnProps) {
		const [copied, setCopied] = useState(false)
		const segments = useMemo(() => groupTurnSegments(turn.assistantMessages), [turn.assistantMessages])
		const lastSegment = segments.at(-1)
		const isSynthetic = useMemo(() => isSyntheticMessage(turn.userMessage), [turn.userMessage])
		const userText = useMemo(() => getUserText(turn.userMessage), [turn.userMessage])

		const responseText = useMemo(() => {
			for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
				const entry = turn.assistantMessages[i]
				if (nativeItemType(entry.info) === "assistantMessage") {
					return assistantOrReasoningText(entry.info)
				}
			}
			return ""
		}, [turn.assistantMessages])

		const onCopy = useCallback(async () => {
			if (!responseText) return
			await navigator.clipboard.writeText(responseText)
			setCopied(true)
			setTimeout(() => setCopied(false), 1200)
		}, [responseText])

		return (
			<div className="group/turn flex flex-col gap-3 py-3" data-turn-id={turn.turnId ?? turn.id}>
				{isUserMessageItem(turn.userMessage.info) && !isSynthetic && (
					<UserMessageBlock
						text={userText}
						canEdit={Boolean(onEditUserMessage)}
						onEdit={
							onEditUserMessage
								? async (next) => {
										await onEditUserMessage(turn.userMessage.info.id, next)
									}
								: undefined
						}
					/>
				)}
				{isSynthetic && (
					<div className="text-center text-[11px] text-muted-foreground">
						{itemDisplayText(turn.userMessage.info)}
					</div>
				)}

				<div className="flex flex-col gap-3">
					{segments.map((segment) =>
						segment.kind === "activity" ? (
							<TurnActivity
								key={segment.id}
								entries={segment.entries}
								working={isWorking && isLast}
							/>
						) : (
							<NativeItemRow
								key={segment.id}
								entry={segment.entry}
								streaming={isWorking && isLast && segment.entry.info.state === "running"}
							/>
						),
					)}
					{isWorking && isLast && !segments.some((segment) => segment.kind === "activity") &&
						(lastSegment?.kind !== "message" || lastSegment.entry.info.state !== "running") && (
							<TurnActivity entries={[]} working />
						)}
				</div>
				{providerErrors.map((row) => (
					<ProviderErrorRow key={row.id} entry={row} />
				))}

				<div className="flex items-center gap-1 opacity-0 transition-opacity group-hover/turn:opacity-100">
					{responseText && (
						<button
							type="button"
							className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-muted"
							onClick={() => void onCopy()}
						>
							<CopyIcon className="size-3.5 stroke-[1.5]" />
							{copied ? "Copied" : "Copy"}
						</button>
					)}
					{onForkFromTurn && turn.turnId && (
						<button
							type="button"
							className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-muted"
							onClick={() => onForkFromTurn(turn.turnId!)}
						>
							<SplitIcon className="size-3.5 stroke-[1.5]" />
							Fork
						</button>
					)}
				</div>
			</div>
		)
	},
	(prev, next) =>
		areTurnsEqual(prev.turn, next.turn) &&
		prev.isLast === next.isLast &&
		prev.isWorking === next.isWorking &&
		prev.providerErrors === next.providerErrors,
)
