import type { ChatTurn } from "../../atoms/derived/session-chat"

/** Index of the last inherited turn; render the fork marker immediately after it. */
export function forkBoundaryAfterTurnIndex(
	turns: ChatTurn[],
	forkFromId: string | undefined,
	atTurnId: string | undefined,
	forkSessionCreatedAt: number,
): number {
	if (!forkFromId || turns.length === 0) return -1

	if (atTurnId) {
		const turnIndex = turns.findIndex((turn) => turn.turnId === atTurnId)
		return turnIndex >= 0 ? turnIndex : -1
	}

	let lastInherited = -1
	for (let index = 0; index < turns.length; index++) {
		const createdAt = turns[index].userMessage.info.createdAt
		const created =
			typeof createdAt === "number"
				? createdAt
				: typeof createdAt === "string"
					? Date.parse(createdAt)
					: Number.NaN
		if (Number.isFinite(created) && created <= forkSessionCreatedAt) {
			lastInherited = index
			continue
		}
		break
	}
	return lastInherited
}
