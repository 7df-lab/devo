import { useEffect, useLayoutEffect, useRef, useState } from "react"
import type { GitDiffStat } from "../../../preload/api"
import { fetchDiffStat, gitCommitAll, gitPush } from "../../services/backend"

interface DialogVisit {
	active: boolean
	pending: boolean
	ready: boolean
	committed: boolean
	uncertain: boolean
	finished: boolean
}

/** Owns one open dialog's scan and commit/push sequence for a fixed directory. */
export function useCommitPushDialog({
	open,
	directory,
	onOpenChange,
	onCommitted,
}: {
	open: boolean
	directory: string
	onOpenChange: (open: boolean) => void
	onCommitted: () => void
}) {
	const visit = useRef<DialogVisit | null>(null)
	const [renderedVisit, setRenderedVisit] = useState<DialogVisit | null>(null)
	const [diffStat, setDiffStat] = useState<GitDiffStat | null>(null)
	const [loadingDiff, setLoadingDiff] = useState(open)
	const [scanError, setScanError] = useState<string | null>(null)
	const [scanAttempt, setScanAttempt] = useState(0)
	const [commitMessage, setCommitMessage] = useState("")
	const [phase, setPhase] = useState<"idle" | "committing" | "pushing">("idle")
	const [committed, setCommitted] = useState(false)
	const [uncertain, setUncertain] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [success, setSuccess] = useState(false)

	useLayoutEffect(() => {
		if (!open) return
		const current: DialogVisit = {
			active: true,
			pending: false,
			ready: false,
			committed: false,
			uncertain: false,
			finished: false,
		}
		visit.current = current
		setRenderedVisit(current)
		setDiffStat(null)
		setLoadingDiff(true)
		setScanError(null)
		setCommitMessage("")
		setPhase("idle")
		setCommitted(false)
		setUncertain(false)
		setError(null)
		setSuccess(false)
		return () => {
			current.active = false
		}
	}, [open])

	const isCurrent = (current: DialogVisit) => current.active && visit.current === current

	// biome-ignore lint/correctness/useExhaustiveDependencies: scanAttempt explicitly retries the read.
	useEffect(() => {
		const current = visit.current
		if (!open || !current) return
		let cancelled = false
		current.ready = false
		setLoadingDiff(true)
		setScanError(null)
		void fetchDiffStat(directory)
			.then((result) => {
				if (cancelled || !current.active || visit.current !== current) return
				current.ready = result.filesChanged > 0
				setDiffStat(result)
			})
			.catch((reason: unknown) => {
				if (cancelled || !current.active || visit.current !== current) return
				setDiffStat(null)
				setScanError(
					reason instanceof Error && reason.message ? reason.message : "Failed to scan changes",
				)
			})
			.finally(() => {
				if (!cancelled && current.active && visit.current === current) setLoadingDiff(false)
			})
		return () => {
			cancelled = true
		}
	}, [open, directory, scanAttempt])

	useEffect(() => {
		const current = visit.current
		if (!open || !success || !current) return
		const timer = setTimeout(() => {
			if (current.active && visit.current === current) {
				current.active = false
				onOpenChange(false)
			}
		}, 1200)
		return () => clearTimeout(timer)
	}, [open, success, onOpenChange])

	const changeOpen = (nextOpen: boolean) => {
		const current = renderedVisit
		if (!current || !isCurrent(current) || current.pending) return
		if (!nextOpen) current.active = false
		onOpenChange(nextOpen)
	}

	const execute = async () => {
		const current = renderedVisit
		if (!current || !isCurrent(current) || current.pending || current.finished || current.uncertain)
			return
		if (!current.committed && !current.ready) return
		current.pending = true
		setError(null)
		try {
			if (!current.committed) {
				setPhase("committing")
				const filesChanged = diffStat?.filesChanged ?? 0
				const message =
					commitMessage.trim() || `Update ${filesChanged} file${filesChanged !== 1 ? "s" : ""}`
				const result = await gitCommitAll(directory, message)
				if (!isCurrent(current)) return
				if (!result.success) {
					setError(`Commit failed: ${result.error || "Unknown error"}`)
					return
				}
				current.committed = true
				setCommitted(true)
				try {
					onCommitted()
				} catch (reason) {
					console.error("Failed to refresh after commit:", reason)
				}
			}
			if (!isCurrent(current)) return
			setPhase("pushing")
			const result = await gitPush(directory)
			if (!isCurrent(current)) return
			if (!result.success) {
				setError(`Committed locally, but push failed: ${result.error || "Unknown error"}`)
				return
			}
			current.finished = true
			setSuccess(true)
		} catch (reason) {
			if (!isCurrent(current)) return
			const detail =
				reason instanceof Error && reason.message ? reason.message : "Connection failed"
			if (current.committed) {
				setError(
					`Committed locally, but the push result could not be confirmed: ${detail}. Check the remote before retrying.`,
				)
			} else {
				current.uncertain = true
				setUncertain(true)
				setError(
					`The commit result could not be confirmed: ${detail}. Check Git history before starting another commit.`,
				)
			}
		} finally {
			current.pending = false
			if (isCurrent(current)) setPhase("idle")
		}
	}

	return {
		diffStat,
		loadingDiff,
		scanError,
		commitMessage,
		setCommitMessage,
		phase,
		committed,
		uncertain,
		error,
		success,
		execute,
		changeOpen,
		retryScan: () => {
			const current = renderedVisit
			if (!current || !isCurrent(current) || current.pending || current.committed) return
			current.ready = false
			setLoadingDiff(true)
			setScanAttempt((attempt) => attempt + 1)
		},
	}
}
