import { useEffect, useLayoutEffect, useRef, useState } from "react"
import type { GitBranchInfo, GitStatusInfo } from "../../preload/api"
import {
	fetchGitBranches,
	fetchGitStatus,
	gitCheckout,
	gitStashAndCheckout,
	isElectron,
} from "../services/backend"

interface BranchOperation {
	branch: string
	phase: "checking" | "dirty" | "warning" | "switching"
	stash: boolean
	status?: GitStatusInfo
}

/** One mounted project owns its reads, confirmations, and checkout admission. */
export function useBranchPicker({
	directory,
	currentBranch,
	activeSessionCount,
	onBranchChanged,
}: {
	directory: string
	currentBranch?: string
	activeSessionCount: number
	onBranchChanged?: (branch: string) => void
}) {
	const [open, setOpen] = useState(false)
	const [branches, setBranches] = useState<GitBranchInfo | null>(null)
	const [loading, setLoading] = useState(false)
	const [loadError, setLoadError] = useState<string | null>(null)
	const [reload, setReload] = useState(0)
	const [operation, setOperation] = useState<BranchOperation | null>(null)
	const [switchError, setSwitchError] = useState<string | null>(null)
	const activeOperation = useRef<BranchOperation | null>(null)
	const mounted = useRef(false)
	const sessionCount = useRef(activeSessionCount)
	useLayoutEffect(() => {
		sessionCount.current = activeSessionCount
	}, [activeSessionCount])

	useLayoutEffect(() => {
		mounted.current = true
		return () => {
			mounted.current = false
			activeOperation.current = null
		}
	}, [])

	// biome-ignore lint/correctness/useExhaustiveDependencies: reload explicitly retries the branch-list read.
	useEffect(() => {
		if (!open || !directory || !isElectron) return
		let cancelled = false
		setLoading(true)
		setLoadError(null)
		void fetchGitBranches(directory)
			.then((result) => {
				if (!cancelled) setBranches(result)
			})
			.catch((error: unknown) => {
				if (cancelled) return
				setBranches(null)
				setLoadError(error instanceof Error ? error.message : "Failed to load branches")
			})
			.finally(() => {
				if (!cancelled) setLoading(false)
			})
		return () => {
			cancelled = true
		}
	}, [open, directory, reload])

	const isCurrent = (request: BranchOperation) =>
		mounted.current && activeOperation.current === request

	const finish = (request: BranchOperation) => {
		if (!isCurrent(request)) return
		activeOperation.current = null
		setOperation(null)
	}

	// Exit animations can retain old dialog controls after a phase transition.
	// Capture this render's request as well as its phase, never a newer request.
	const renderedRequest = activeOperation.current
	const confirmation = (phase: "dirty" | "warning") =>
		renderedRequest &&
		operation?.phase === phase &&
		renderedRequest.phase === phase &&
		isCurrent(renderedRequest)
			? renderedRequest
			: null

	const checkout = async (request: BranchOperation) => {
		if (!isCurrent(request) || request.phase === "switching") return
		request.phase = "switching"
		setOperation({ ...request })
		try {
			const result = request.stash
				? await gitStashAndCheckout(directory, request.branch)
				: await gitCheckout(directory, request.branch)
			if (!isCurrent(request)) return
			if (!result.success) {
				setSwitchError(result.error ?? "Checkout failed")
				return
			}
			setBranches((previous) =>
				previous
					? {
							...previous,
							current: request.branch,
							state: "branch",
							detached: false,
						}
					: null,
			)
			onBranchChanged?.(request.branch)
		} catch (error) {
			if (isCurrent(request)) {
				setSwitchError(error instanceof Error ? error.message : "Checkout failed")
			}
		} finally {
			finish(request)
		}
	}

	const selectBranch = async (branch: string) => {
		if (!mounted.current || activeOperation.current) return
		const localName =
			branch.includes("/") && !branches?.local.includes(branch)
				? branch.replace(/^[^/]+\//, "")
				: branch
		setOpen(false)
		if (localName === (branches?.current || currentBranch || "")) return

		const request: BranchOperation = {
			branch: localName,
			phase: "checking",
			stash: false,
		}
		activeOperation.current = request
		setOperation({ ...request })
		setSwitchError(null)
		try {
			const status = await fetchGitStatus(directory)
			if (!isCurrent(request)) return
			if (!status.isClean) {
				request.phase = "dirty"
				request.status = status
				setOperation({ ...request })
			} else if (sessionCount.current > 0) {
				request.phase = "warning"
				setOperation({ ...request })
			} else {
				await checkout(request)
			}
		} catch (error) {
			if (!isCurrent(request)) return
			setSwitchError(error instanceof Error ? error.message : "Failed to check status")
			finish(request)
		}
	}

	const stashAndSwitch = () => {
		const request = confirmation("dirty")
		if (!request) return
		request.stash = true
		if (sessionCount.current > 0) {
			request.phase = "warning"
			setOperation({ ...request })
		} else {
			void checkout(request)
		}
	}

	const proceed = () => {
		const request = confirmation("warning")
		if (!request) return
		void checkout(request)
	}

	return {
		open,
		setOpen,
		branches,
		loading,
		loadError,
		retryLoad: () => setReload((value) => value + 1),
		operation,
		switchError,
		dismissError: () => setSwitchError(null),
		selectBranch,
		stashAndSwitch,
		proceed,
		cancelDirty: () => {
			const request = confirmation("dirty")
			if (request) finish(request)
		},
		cancelWarning: () => {
			const request = confirmation("warning")
			if (request) finish(request)
		},
	}
}
