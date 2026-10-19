/** Delete all folder sessions with bounded concurrency and report failures after settling every request. */
export async function deleteProjectSessionBatch({
	sessionIds,
	deleteSession,
	onDeleted,
}: {
	sessionIds: readonly string[]
	deleteSession: (sessionId: string) => Promise<void>
	onDeleted: (sessionId: string) => void
}): Promise<void> {
	const pending = sessionIds.values()
	const errors: unknown[] = []
	await Promise.all(
		Array.from({ length: Math.min(sessionIds.length, 8) }, async () => {
			for (const sessionId of pending) {
				try {
					await deleteSession(sessionId)
					onDeleted(sessionId)
				} catch (error) {
					errors.push(error)
				}
			}
		}),
	)
	if (errors.length) {
		const detail = errors[0] instanceof Error ? errors[0].message : String(errors[0])
		throw new AggregateError(errors, `${errors.length} session deletion(s) failed: ${detail}`)
	}
}
