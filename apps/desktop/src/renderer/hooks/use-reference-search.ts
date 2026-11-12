/** Server-backed composer `@` search, scoped to the current popup/query/runtime. */
import type { ReferenceSearchResult, ReferenceSearchSnapshot } from "@devo-ai/sdk/v2/client"
import { useAtomValue } from "jotai"
import { useLayoutEffect, useState } from "react"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { appStore } from "../atoms/store"
import { getProjectClient } from "../services/connection-manager"

const REFERENCE_SEARCH_DEBOUNCE_MS = 150
const EMPTY_RESULTS: ReferenceSearchResult[] = []

export function useReferenceSearch(directory: string | null, query: string, enabled = true) {
	const connected = useAtomValue(serverConnectedAtom)
	const serverUrl = useAtomValue(serverUrlAtom)
	const serverId = useAtomValue(activeServerIdAtom)
	const [generation, setGeneration] = useState(0)
	const key = JSON.stringify([
		directory,
		query,
		enabled,
		connected,
		serverUrl,
		serverId,
		generation,
	])
	const [state, setState] = useState<{
		key: string
		results: ReferenceSearchResult[]
		isLoading: boolean
		error: string | null
	}>()

	useLayoutEffect(() => {
		if (!directory || !enabled || !connected) {
			setState(undefined)
			return
		}
		setState({ key, results: EMPTY_RESULTS, isLoading: true, error: null })
		const client = getProjectClient(directory)
		if (!client) {
			setState({ key, results: EMPTY_RESULTS, isLoading: false, error: null })
			return
		}
		let current = true
		let unsubscribe = () => {}
		const invalidate = () => {
			current = false
			setGeneration((value) => value + 1)
		}
		// Disconnect/reconnect may be batched into one render; invalidate the
		// request synchronously even if the final connected value is unchanged.
		const stops = [serverConnectedAtom, serverUrlAtom, activeServerIdAtom].map((atom) =>
			appStore.sub(atom, invalidate),
		)
		const timer = setTimeout(() => {
			if (!current) return
			const applySnapshot = (snapshot: ReferenceSearchSnapshot) => {
				if (!current || snapshot.query !== query) return
				setState({
					key,
					results: snapshot.results,
					isLoading: !snapshot.file_search_complete,
					error: client.referenceSearch.getState().error,
				})
			}
			unsubscribe = client.referenceSearch.subscribe(applySnapshot)
			void client.referenceSearch.startOrUpdate({ query }).catch((cause: unknown) => {
				if (!current) return
				setState({
					key,
					results: EMPTY_RESULTS,
					isLoading: false,
					error: cause instanceof Error ? cause.message : "reference search failed",
				})
			})
		}, REFERENCE_SEARCH_DEBOUNCE_MS)
		return () => {
			current = false
			clearTimeout(timer)
			unsubscribe()
			for (const stop of stops) stop()
			void client.referenceSearch.cancel()
		}
	}, [directory, query, enabled, connected, key])

	const eligible = Boolean(directory && enabled && connected)
	const visible = eligible && state?.key === key ? state : null
	return {
		results: visible?.results ?? EMPTY_RESULTS,
		isLoading: visible?.isLoading ?? eligible,
		error: visible?.error ?? null,
	}
}
