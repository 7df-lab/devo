/** Show an initial transcript skeleton only while no turns are available. */
export function shouldShowChatLoadingSkeleton(loading: boolean, visibleTurnCount: number): boolean {
	return loading && visibleTurnCount === 0
}
