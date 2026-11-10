import { useAtomValue } from "jotai"
import { useLayoutEffect, useRef, useState } from "react"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { sessionFamily, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { fetchSessionById } from "../services/connection-manager"

export type SessionResolution =
	| { status: "idle" | "loading" | "offline" | "notFound" }
	| { status: "error"; message: string }

/** Resolve cold child/deep-link identities without resuming work or scanning roots. */
export function useSessionResolution({
	sessionId,
	known,
	active,
}: {
	sessionId: string
	known: boolean
	active: boolean
}) {
	const connected = useAtomValue(serverConnectedAtom)
	const serverUrl = useAtomValue(serverUrlAtom)
	const serverId = useAtomValue(activeServerIdAtom)
	const key = JSON.stringify([sessionId, serverUrl, serverId, connected, active, known])

	const [result, setResult] = useState<{ key: string; state: SessionResolution }>()
	const retry = useRef(() => {})

	// A layout lifetime is stopped when Suspense hides this view and restarted
	// on reveal, even if its props have not changed. Abandoned renders do neither.
	useLayoutEffect(() => {
		if (!sessionId || !active || known || !connected) return
		let current = true
		let pending = false
		const isCurrent = () =>
			current &&
			appStore.get(serverConnectedAtom) &&
			appStore.get(serverUrlAtom) === serverUrl &&
			appStore.get(activeServerIdAtom) === serverId
		const run = async () => {
			if (!isCurrent() || pending) return
			pending = true
			setResult({ key, state: { status: "loading" } })
			try {
				const session = await fetchSessionById(sessionId)
				if (!isCurrent()) return
				if (!session) {
					setResult({ key, state: { status: "notFound" } })
					return
				}
				if (session.id !== sessionId) throw new Error("The runtime returned a different session.")
				// A live event may have populated the store before React has rerendered.
				if (!appStore.get(sessionFamily(sessionId)))
					appStore.set(upsertSessionAtom, { session, directory: session.directory ?? "" })
				setResult({ key, state: { status: "idle" } })
			} catch (cause) {
				if (isCurrent())
					setResult({
						key,
						state: {
							status: "error",
							message: cause instanceof Error ? cause.message : String(cause),
						},
					})
			} finally {
				pending = false
			}
		}
		retry.current = () => {
			void run()
		}
		void run()
		return () => {
			current = false
			retry.current = () => {}
		}
	}, [sessionId, active, known, connected, key, serverUrl, serverId])

	const state: SessionResolution =
		!sessionId || !active || known
			? { status: "idle" }
			: !connected
				? { status: "offline" }
				: result?.key === key
					? result.state
					: { status: "loading" }
	return { state, retry: () => retry.current() }
}
