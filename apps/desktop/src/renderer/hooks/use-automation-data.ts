/**
 * Single data gateway hook for the automations subsystem.
 *
 * Fetches automations + runs on mount, subscribes to real-time push
 * updates via the preload bridge, and polls every 30s for timer freshness.
 * This is the ONLY writer to automationsAtom / automationRunsAtom.
 *
 * Mount once in AutomationsPage; everything else reads from atoms.
 */

import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useRef, useState } from "react"
import { automationRunsAtom, setAutomationRunsAtom, setAutomationsAtom } from "../atoms/automations"
import { createLogger } from "../lib/logger"
import { fetchAutomationRuns, fetchAutomations } from "../services/backend"

const log = createLogger("use-automation-data")

/** Polling interval for countdown/timer freshness (30 seconds). */
const POLL_INTERVAL = 30_000

const isElectron = typeof window !== "undefined" && "devo" in window

export function useAutomationData(selectedRunId?: string) {
	const runs = useAtomValue(automationRunsAtom)
	const awaitingSession = runs.some((run) =>
		run.id === selectedRunId && run.status === "running" && !run.sessionId,
	)
	const pollInterval = awaitingSession ? 2_000 : POLL_INTERVAL
	const setAutomations = useSetAtom(setAutomationsAtom)
	const setRuns = useSetAtom(setAutomationRunsAtom)
	const [loading, setLoading] = useState(true)
	const [error, setError] = useState<string | null>(null)
	const mountedRef = useRef(false)
	const fetchCountRef = useRef(0)
	const requestPendingRef = useRef(false)
	const prevRunCountRef = useRef<number | null>(null)
	const prevRunStatusMapRef = useRef<Map<string, string>>(new Map())

	const refetch = useCallback(
		async (source = "unknown") => {
			if (!mountedRef.current || !isElectron || (source === "poll" && requestPendingRef.current)) return
			requestPendingRef.current = true
			const fetchId = ++fetchCountRef.current
			log.debug(`Fetching automation data (#${fetchId}, source: ${source})`)
			try {
				const [automations, runs] = await Promise.all([fetchAutomations(), fetchAutomationRuns()])
				if (!mountedRef.current || fetchId !== fetchCountRef.current) return

				// Log automation schedule state for debugging
				for (const a of automations) {
					if (a.status === "active") {
						log.debug("Active automation", {
							id: a.id,
							name: a.name,
							nextRunAt: a.nextRunAt ? new Date(a.nextRunAt).toISOString() : "none",
							lastRunAt: a.lastRunAt ? new Date(a.lastRunAt).toISOString() : "never",
							runCount: a.runCount,
							consecutiveFailures: a.consecutiveFailures,
						})
					}
				}

				// Detect new runs and status changes
				const prevMap = prevRunStatusMapRef.current
				const newMap = new Map<string, string>()
				for (const run of runs) {
					newMap.set(run.id, run.status)
					const prevStatus = prevMap.get(run.id)
					if (!prevStatus) {
						log.info("New automation run detected", {
							runId: run.id,
							automationId: run.automationId,
							status: run.status,
							workspace: run.workspace,
						})
					} else if (prevStatus !== run.status) {
						log.info("Automation run status changed", {
							runId: run.id,
							automationId: run.automationId,
							from: prevStatus,
							to: run.status,
						})
					}
				}
				prevRunStatusMapRef.current = newMap

				if (prevRunCountRef.current !== null && runs.length !== prevRunCountRef.current) {
					log.info("Run count changed", {
						from: prevRunCountRef.current,
						to: runs.length,
					})
				}
				prevRunCountRef.current = runs.length

				setAutomations(automations)
				setRuns(runs)
				setError(null)
			} catch (err) {
				if (!mountedRef.current || fetchId !== fetchCountRef.current) return
				const msg = err instanceof Error ? err.message : "Failed to load automations"
				setError(msg)
				log.error("Failed to fetch automation data", { fetchId, source, error: msg })
			} finally {
				if (mountedRef.current && fetchId === fetchCountRef.current) {
					requestPendingRef.current = false
					setLoading(false)
				}
			}
		},
		[setAutomations, setRuns],
	)

	// Subscribe before the initial read so a push cannot fall into a setup gap.
	useEffect(() => {
		mountedRef.current = true
		setLoading(isElectron)
		if (!isElectron) return () => { mountedRef.current = false }
		log.info("Automation data gateway mounted, starting polling", {
			pollIntervalMs: POLL_INTERVAL,
		})
		const unsub = window.devo?.onAutomationRunsUpdated(() => {
			log.info("Received automation:runs-updated from main process")
			refetch("ipc-push")
		})
		refetch("mount")

		return () => {
			mountedRef.current = false
			// Invalidate outstanding reads even if StrictMode immediately sets up again.
			fetchCountRef.current++
			requestPendingRef.current = false
			unsub?.()
			log.info("Automation data gateway unmounted")
		}
	}, [refetch])

	// The detail view uses this same gateway, not a competing runs-only writer.
	useEffect(() => {
		if (!isElectron) return
		const intervalId = setInterval(() => { void refetch("poll") }, pollInterval)
		return () => clearInterval(intervalId)
	}, [pollInterval, refetch])

	return { loading, error, refetch: () => refetch("manual") }
}
