/**
 * Hook for server-backed composer `@` reference search.
 *
 * Preserves the server-ranked Skill, MCP, and File result stream from the
 * connection-local `search/*` session.
 */
import type { ReferenceSearchResult, ReferenceSearchSnapshot } from "@devo-ai/sdk/v2/client"
import { useEffect, useRef, useState } from "react"
import { getProjectClient } from "../services/connection-manager"

const REFERENCE_SEARCH_DEBOUNCE_MS = 150

export function useReferenceSearch(directory: string | null, query: string, enabled = true) {
	const [debouncedQuery, setDebouncedQuery] = useState(query)
	const [results, setResults] = useState<ReferenceSearchResult[]>([])
	const [isLoading, setIsLoading] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

	useEffect(() => {
		if (timerRef.current) clearTimeout(timerRef.current)
		timerRef.current = setTimeout(() => {
			setDebouncedQuery(query)
		}, REFERENCE_SEARCH_DEBOUNCE_MS)
		return () => {
			if (timerRef.current) clearTimeout(timerRef.current)
		}
	}, [query])

	useEffect(() => {
		if (!directory || !enabled) {
			setResults([])
			setIsLoading(false)
			setError(null)
			return
		}

		const client = getProjectClient(directory)
		if (!client) {
			setResults([])
			setIsLoading(false)
			setError(null)
			return
		}

		let cancelled = false
		const applySnapshot = (snapshot: ReferenceSearchSnapshot) => {
			if (cancelled) return
			setResults(snapshot.results)
			setIsLoading(!snapshot.file_search_complete)
			setError(client.referenceSearch.getState().error)
		}

		const unsubscribe = client.referenceSearch.subscribe(applySnapshot)
		setIsLoading(true)
		setError(null)
		void client.referenceSearch.startOrUpdate({ query: debouncedQuery }).catch((searchError: unknown) => {
			if (cancelled) return
			setResults([])
			setIsLoading(false)
			setError(searchError instanceof Error ? searchError.message : "reference search failed")
		})

		return () => {
			cancelled = true
			unsubscribe()
			void client.referenceSearch.cancel()
		}
	}, [directory, debouncedQuery, enabled])

	return {
		results,
		isLoading,
		error,
	}
}
