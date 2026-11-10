import { projectNativeSession } from "@devo-ai/sdk/v2/session-projection"
import { useAtomValue } from "jotai"
import { useLayoutEffect, useRef, useState } from "react"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { isMockModeAtom } from "../atoms/mock-mode"
import { sessionFamily, setSessionsAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { getBaseClient } from "../services/connection-manager"

export type PaletteSearchStatus = "idle" | "loading" | "offline" | "error" | "ready"

/** Search belongs to the visible palette visit, query and connected runtime. */
export function usePaletteSessionSearch(open: boolean, query: string) {
	const connected = useAtomValue(serverConnectedAtom)
	const serverUrl = useAtomValue(serverUrlAtom)
	const serverId = useAtomValue(activeServerIdAtom)
	const mockMode = useAtomValue(isMockModeAtom)
	const search = query.trim()
	const [generation, setGeneration] = useState(0)
	const key = JSON.stringify([open, search, connected, serverUrl, serverId, mockMode, generation])
	const [result, setResult] = useState<{ key: string; status: PaletteSearchStatus }>()
	const retry = useRef(() => {})

	useLayoutEffect(() => {
		if (!open || search.length < 2 || mockMode) return
		let current = true
		let pending = false
		let timer: ReturnType<typeof setTimeout> | undefined
		// Invalidate synchronously: a disconnect/reconnect can happen in a single
		// React batch, leaving the rendered connected value unchanged.
		const invalidate = () => {
			current = false
			clearTimeout(timer)
			setGeneration((value) => value + 1)
		}
		const unsubscribe = [
			serverConnectedAtom,
			serverUrlAtom,
			activeServerIdAtom,
			isMockModeAtom,
		].map((atom) => appStore.sub(atom, invalidate))
		const eligible = () =>
			current &&
			appStore.get(serverConnectedAtom) &&
			!appStore.get(isMockModeAtom) &&
			appStore.get(serverUrlAtom) === serverUrl &&
			appStore.get(activeServerIdAtom) === serverId
		const run = async () => {
			if (!eligible() || pending) return
			pending = true
			clearTimeout(timer)
			setResult({ key, status: "loading" })
			const client = getBaseClient()
			const isCurrent = () => eligible() && getBaseClient() === client
			try {
				if (!client) throw new Error("Runtime unavailable")
				// Unlike session.list, roster does not share abandoned in-flight queries
				// or populate SDK caches/usage events before we can check the lifetime.
				const page = await client.session.roster({ cwds: [], search, limit: 50 })
				if (!isCurrent()) return
				const sessions = page.data
					.slice(0, 50)
					.map((info) =>
						projectNativeSession(info, appStore.get(sessionFamily(info.id))?.session, info.cwd),
					)
				// Preserve live status, loaded local sessions and sidebar pagination.
				appStore.set(setSessionsAtom, { sessions, statuses: {}, directory: "" })
				setResult({ key, status: "ready" })
			} catch {
				if (isCurrent()) setResult({ key, status: "error" })
			} finally {
				pending = false
			}
		}
		retry.current = () => {
			void run()
		}
		if (connected) {
			setResult({ key, status: "loading" })
			timer = setTimeout(() => {
				void run()
			}, 250)
		}
		return () => {
			current = false
			clearTimeout(timer)
			retry.current = () => {}
			for (const stop of unsubscribe) stop()
		}
	}, [open, search, connected, serverUrl, serverId, mockMode, key])

	const status: PaletteSearchStatus =
		!open || search.length < 2 || mockMode
			? "idle"
			: !connected
				? "offline"
				: result?.key === key
					? result.status
					: "loading"
	return { status, retry: () => retry.current() }
}
