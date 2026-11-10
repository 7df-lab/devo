import type { WorkspaceChangeScope, WorkspaceChangeView } from "@devo-ai/sdk/v2/client"
import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useMemo } from "react"
import { isMockModeAtom } from "../atoms/mock-mode"
import { appStore } from "../atoms/store"
import {
	latestWorkspaceTurnIdFamily,
	markWorkspaceChangesLoadingAtom,
	registerSessionWorkspaceDirectoryAtom,
	setWorkspaceChangesErrorAtom,
	setWorkspaceChangesViewAtom,
	workspaceChangesKey,
	workspaceChangesStateFamily,
} from "../atoms/workspace-changes"
import { numberFromProtocol, isCompletePatch } from "../lib/workspace-diff"
import { getProjectClient } from "../services/connection-manager"
import { getWorkspaceChanges } from "../services/devo"
import { workspaceChangeRequests } from "./workspace-change-requests"

const FULL_DIFF_LIMIT_BYTES = 2_000_000

/** Sibling scopes warmed after the active Summary paints. */
const PREFETCH_SCOPES: WorkspaceChangeScope[] = ["uncommitted", "unstaged", "staged", "branch"]

export type ScopeStatsPreview = {
	additions: number
	deletions: number
}

type FetchArgs = {
	sessionId: string
	directory: string
	scope: WorkspaceChangeScope
	baseBranch?: string | null
	turnId?: string | null
	ignoreWhitespace: boolean
}

function cacheKeyFor(args: {
	sessionId: string
	directory: string
	scope: WorkspaceChangeScope
	turnId?: string | null
	baseBranch?: string | null
	ignoreWhitespace: boolean
}): string {
	return workspaceChangesKey({
		cwd: args.directory,
		sessionId: args.sessionId,
		scope: args.scope,
		turnId: args.turnId ?? undefined,
		baseBranch: args.baseBranch,
		ignoreWhitespace: args.ignoreWhitespace,
	})
}

function needsSummary(key: string): boolean {
	const state = appStore.get(workspaceChangesStateFamily(key))
	return !state.view || state.stale
}

function hasFilePatch(view: WorkspaceChangeView, filePath: string): boolean {
	const normalized = filePath.replace(/\\/g, "/")
	const existing = view.unifiedDiff ?? ""
	if (
		!(
			existing.includes(`b/${normalized}\n`) ||
			existing.includes(`b/${normalized}\r\n`) ||
			existing.endsWith(`b/${normalized}`)
		)
	) {
		return false
	}
	// Header-only stubs include `b/path` but are not complete patches.
	const chunk = extractFilePatch(existing, normalized)
	return isCompletePatch(chunk)
}

function extractFilePatch(diff: string, normalizedPath: string): string | null {
	const parts = diff.split(/(?=^diff --git )/m)
	for (const part of parts) {
		if (
			part.includes(`b/${normalizedPath}\n`) ||
			part.includes(`b/${normalizedPath}\r\n`) ||
			part.trimEnd().endsWith(`b/${normalizedPath}`)
		) {
			return part
		}
	}
	return null
}

function stripFilePatch(diff: string, filePath: string): string {
	const normalized = filePath.replace(/\\/g, "/")
	const parts = diff.split(/(?=^diff --git )/m)
	return parts
		.filter((part) => {
			const trimmed = part.trimStart()
			if (!trimmed) return false
			return !(
				trimmed.includes(`b/${normalized}\n`) ||
				trimmed.includes(`b/${normalized}\r\n`) ||
				trimmed.trimEnd().endsWith(`b/${normalized}`)
			)
		})
		.join("")
		.replace(/^\n+/, "")
}

/** True when every non-binary file already has a complete patch chunk. */
function viewHasCompletePatches(view: WorkspaceChangeView): boolean {
	if (view.files.length === 0) return Boolean(view.unifiedDiff != null)
	const diff = view.unifiedDiff
	if (diff == null || diff.length === 0) return false
	return view.files.every((file) => Boolean(file.binary) || hasFilePatch(view, String(file.path)))
}

function findFile(view: WorkspaceChangeView, filePath: string) {
	const normalized = filePath.replace(/\\/g, "/")
	return view.files.find((file) => String(file.path).replace(/\\/g, "/") === normalized)
}

function fileHasSides(view: WorkspaceChangeView, filePath: string): boolean {
	const file = findFile(view, filePath)
	if (!file) return false
	return typeof file.oldText === "string" || typeof file.newText === "string"
}

/** Ready for MultiFileDiff, or PatchDiff/binary fallback without another sides fetch. */
function fileExpandReady(
	view: WorkspaceChangeView,
	filePath: string,
	sidesFetched: Set<string>,
): boolean {
	const normalized = filePath.replace(/\\/g, "/")
	const file = findFile(view, filePath)
	if (!file) return false
	if (file.binary) return true
	if (fileHasSides(view, filePath)) return true
	// Path-scoped sides already attempted (oversize/unavailable) — stop refetching.
	if (sidesFetched.has(normalized)) return true
	return false
}

export function mergePathFullIntoView(
	base: WorkspaceChangeView,
	patchView: WorkspaceChangeView,
): WorkspaceChangeView {
	const files = base.files.map((file) => {
		const path = String(file.path).replace(/\\/g, "/")
		const updated = patchView.files.find((item) => String(item.path).replace(/\\/g, "/") === path)
		if (!updated) return file
		return {
			...file,
			status: updated.status ?? file.status,
			additions: updated.additions ?? file.additions,
			deletions: updated.deletions ?? file.deletions,
			binary: updated.binary ?? file.binary,
			diffTruncated: updated.diffTruncated ?? file.diffTruncated,
			oldText: updated.oldText ?? file.oldText,
			newText: updated.newText ?? file.newText,
		}
	})
	// Keep Summary-only rows (e.g. untracked) that Full omitted.
	for (const incoming of patchView.files) {
		const path = String(incoming.path).replace(/\\/g, "/")
		if (files.some((file) => String(file.path).replace(/\\/g, "/") === path)) continue
		files.push(incoming)
	}
	const incoming = (patchView.unifiedDiff ?? "").trimEnd()
	const paths = patchView.files.map((file) => String(file.path).replace(/\\/g, "/"))
	if (!incoming) return { ...base, files }
	let baseDiff = base.unifiedDiff ?? ""
	for (const path of paths) {
		if (!hasFilePatch(base, path)) {
			baseDiff = stripFilePatch(baseDiff, path)
		}
	}
	const already = paths.every((path) => hasFilePatch(base, path))
	if (already) return { ...base, files }
	const unified = baseDiff.trimEnd() ? `${baseDiff.trimEnd()}\n${incoming}\n` : `${incoming}\n`
	return { ...base, files, unifiedDiff: unified }
}

export function useWorkspaceChanges(
	sessionId: string,
	directory: string,
	scope: WorkspaceChangeScope,
	options: {
		enabled?: boolean
		baseBranch?: string | null
		ignoreWhitespace?: boolean
		prefetch?: boolean
	} = {},
) {
	const latestTurnId = useAtomValue(latestWorkspaceTurnIdFamily(sessionId))
	const turnId = scope === "turn" ? latestTurnId : undefined
	const ignoreWhitespace = options.ignoreWhitespace ?? false
	const key = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope,
				turnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[sessionId, directory, scope, turnId, options.baseBranch, ignoreWhitespace],
	)
	const state = useAtomValue(workspaceChangesStateFamily(key))
	const fallbackKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope,
				turnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace: !ignoreWhitespace,
			}),
		[sessionId, directory, scope, turnId, options.baseBranch, ignoreWhitespace],
	)
	const fallbackState = useAtomValue(workspaceChangesStateFamily(fallbackKey))
	const markLoading = useSetAtom(markWorkspaceChangesLoadingAtom)
	const setView = useSetAtom(setWorkspaceChangesViewAtom)
	const setError = useSetAtom(setWorkspaceChangesErrorAtom)
	const registerDirectory = useSetAtom(registerSessionWorkspaceDirectoryAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const enabled = options.enabled ?? true
	const prefetch = options.prefetch ?? true

	useEffect(() => {
		if (sessionId && directory) {
			registerDirectory({ sessionId, directory })
		}
	}, [directory, registerDirectory, sessionId])

	const fetchDetails = useCallback(
		async (args: FetchArgs, filePath?: string) => {
			if (isMockMode) return
			const fetchKey = cacheKeyFor(args)
			const cache = workspaceChangesStateFamily(fetchKey)
			const requests = workspaceChangeRequests(cache)
			const current = appStore.get(cache).view
			if (!current) return
			const normalizedPath = filePath?.replace(/\\/g, "/")
			// Remember intent even during Refresh; the new Summary will reload it.
			if (normalizedPath) {
				requests.requestedPaths.add(normalizedPath)
			} else {
				for (const file of current.files) {
					requests.requestedPaths.add(String(file.path).replace(/\\/g, "/"))
				}
			}
			if (requests.summaryPending) return
			if (normalizedPath) {
				if (!findFile(current, normalizedPath)) return
				if (fileExpandReady(current, normalizedPath, requests.sidesFetched)) return
			} else if (viewHasCompletePatches(current)) return

			const client = getProjectClient(args.directory)
			if (!client) {
				setError({ key: fetchKey, error: "No workspace connection for this project" })
				return
			}
			const flightKey = normalizedPath ?? ""
			if (requests.detailsPending.has(flightKey)) return
			requests.detailsPending.add(flightKey)
			const generation = requests.generation
			try {
				const result = await getWorkspaceChanges(client, {
					sessionId: args.sessionId,
					scopes: [args.scope],
					cwd: args.directory || undefined,
					baseBranch: args.baseBranch ?? undefined,
					turnId: args.turnId ?? undefined,
					diffDetail: "full",
					maxDiffBytes: FULL_DIFF_LIMIT_BYTES,
					ignoreWhitespace: args.ignoreWhitespace || undefined,
					paths: normalizedPath ? [normalizedPath] : undefined,
					includeFileSides: Boolean(normalizedPath),
				})
				if (generation !== requests.generation) return
				const patchView = result.views.find((item) => item.scope === args.scope)
				if (!patchView) {
					requests.detailErrors.set(flightKey, "Workspace change view missing from response")
					setError({ key: fetchKey, error: "Workspace change view missing from response" })
					return
				}
				if (normalizedPath) requests.sidesFetched.add(normalizedPath)
				const latest = appStore.get(cache).view
				if (!latest) return
				requests.detailErrors.delete(flightKey)
				setView({ key: fetchKey, view: mergePathFullIntoView(latest, patchView) })
				// A successful sibling must not hide another row's failed load/Retry.
				const remainingError = requests.detailErrors.values().next().value
				if (remainingError) setError({ key: fetchKey, error: remainingError })
			} catch (error) {
				if (generation !== requests.generation) return
				const message = error instanceof Error ? error.message : "Failed to load file diff"
				requests.detailErrors.set(flightKey, message)
				setError({ key: fetchKey, error: message })
			} finally {
				// An obsolete request must not release its replacement's flight slot.
				if (generation === requests.generation) requests.detailsPending.delete(flightKey)
			}
		},
		[isMockMode, setError, setView],
	)

	const fetchServer = useCallback(
		async (args: FetchArgs) => {
			if (isMockMode) return
			const client = getProjectClient(args.directory)
			const fetchKey = cacheKeyFor(args)
			const cache = workspaceChangesStateFamily(fetchKey)
			const requests = workspaceChangeRequests(cache)
			if (requests.summaryPending) return
			if (!client) {
				setError({
					key: fetchKey,
					error: args.directory
						? "No workspace connection for this project"
						: "Open a project to load changes",
				})
				return
			}
			requests.summaryPending = true
			requests.generation += 1
			const generation = requests.generation
			requests.detailsPending.clear()
			markLoading({ key: fetchKey, loading: true, error: null })
			try {
				const result = await getWorkspaceChanges(client, {
					sessionId: args.sessionId,
					scopes: [args.scope],
					cwd: args.directory || undefined,
					baseBranch: args.baseBranch ?? undefined,
					turnId: args.turnId ?? undefined,
					diffDetail: "summary",
					ignoreWhitespace: args.ignoreWhitespace || undefined,
				})
				const view = result.views.find((item) => item.scope === args.scope)
				if (!view) {
					setError({ key: fetchKey, error: "Workspace change view missing from response" })
					return
				}
				// Summary is authoritative. Equal stats do not imply equal content.
				// Keep the visible old view while loading, never carry its sides forward.
				requests.sidesFetched.clear()
				requests.detailErrors.clear()
				for (const path of requests.requestedPaths) {
					if (!findFile(view, path)) requests.requestedPaths.delete(path)
				}
				setView({ key: fetchKey, view })
			} catch (error) {
				setError({
					key: fetchKey,
					error: error instanceof Error ? error.message : "Failed to load workspace changes",
				})
				return
			} finally {
				requests.summaryPending = false
			}
			// ReviewPanel loads on expand, not on view changes. Refresh only paths
			// already requested, including expand clicks made during the Summary.
			const paths = [...requests.requestedPaths]
			const concurrency = 4
			for (let i = 0; i < paths.length; i += concurrency) {
				if (generation !== requests.generation) return
				await Promise.all(paths.slice(i, i + concurrency).map((path) => fetchDetails(args, path)))
			}
		},
		[fetchDetails, isMockMode, markLoading, setError, setView],
	)

	const activeArgs = useMemo<FetchArgs>(
		() => ({
			sessionId,
			directory,
			scope,
			baseBranch: options.baseBranch,
			turnId,
			ignoreWhitespace,
		}),
		[directory, ignoreWhitespace, options.baseBranch, scope, sessionId, turnId],
	)

	const fetchChanges = useCallback(async () => {
		await fetchServer(activeArgs)
	}, [activeArgs, fetchServer])

	/** Expand lazily; whole-tree Full is followed by per-path sides in Expand all. */
	const ensureFullPatches = useCallback(
		async (filePath?: string) => {
			if (!enabled) return
			await fetchDetails(activeArgs, filePath)
		},
		[activeArgs, enabled, fetchDetails],
	)

	// First open is Summary-only. Previously requested paths reload after refresh.
	useEffect(() => {
		if (!enabled) return
		let cancelled = false
		void (async () => {
			if (needsSummary(key)) {
				await fetchServer(activeArgs)
				if (cancelled) return
			}
			if (prefetch && !isMockMode && directory) {
				for (const nextScope of PREFETCH_SCOPES) {
					if (cancelled) return
					if (nextScope === scope) continue
					const warmArgs: FetchArgs = {
						sessionId,
						directory,
						scope: nextScope,
						baseBranch: options.baseBranch,
						ignoreWhitespace,
					}
					const warmKey = cacheKeyFor(warmArgs)
					if (!needsSummary(warmKey)) continue
					void fetchServer(warmArgs)
				}
			}
		})()
		return () => {
			cancelled = true
		}
	}, [
		activeArgs,
		directory,
		enabled,
		fetchServer,
		ignoreWhitespace,
		isMockMode,
		key,
		options.baseBranch,
		prefetch,
		scope,
		sessionId,
	])

	const displayView = state.view ?? (state.loading ? fallbackState.view : null)
	const isInitialLoading = !displayView && state.loading
	const isRefreshing = Boolean(displayView) && state.loading
	const patchesPending = Boolean(
		displayView && displayView.files.length > 0 && !displayView.unifiedDiff,
	)

	return {
		...state,
		view: displayView,
		key,
		latestTurnId,
		isInitialLoading,
		isRefreshing,
		patchesPending,
		refetch: fetchChanges,
		ensureFullPatches,
	}
}

/** Read cached Summary stats for every primary scope (dropdown preview). */
export function useWorkspaceScopeStatsPreview(
	sessionId: string,
	directory: string,
	options: {
		enabled?: boolean
		baseBranch?: string | null
		ignoreWhitespace?: boolean
	} = {},
): Partial<Record<WorkspaceChangeScope, ScopeStatsPreview>> {
	const latestTurnId = useAtomValue(latestWorkspaceTurnIdFamily(sessionId))
	const ignoreWhitespace = options.ignoreWhitespace ?? false
	const enabled = options.enabled ?? true

	const turnKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "turn",
				turnId: latestTurnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, latestTurnId, options.baseBranch, sessionId],
	)
	const uncommittedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "uncommitted",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const stagedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "staged",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const unstagedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "unstaged",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const branchKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "branch",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)

	const turn = useAtomValue(workspaceChangesStateFamily(turnKey))
	const uncommitted = useAtomValue(workspaceChangesStateFamily(uncommittedKey))
	const staged = useAtomValue(workspaceChangesStateFamily(stagedKey))
	const unstaged = useAtomValue(workspaceChangesStateFamily(unstagedKey))
	const branch = useAtomValue(workspaceChangesStateFamily(branchKey))

	return useMemo(() => {
		if (!enabled) return {}
		const out: Partial<Record<WorkspaceChangeScope, ScopeStatsPreview>> = {}
		const take = (scope: WorkspaceChangeScope, view: WorkspaceChangeView | null | undefined) => {
			if (!view) return
			out[scope] = {
				additions: numberFromProtocol(view.stats?.additions) ?? 0,
				deletions: numberFromProtocol(view.stats?.deletions) ?? 0,
			}
		}
		take("turn", turn.view)
		take("uncommitted", uncommitted.view)
		take("staged", staged.view)
		take("unstaged", unstaged.view)
		take("branch", branch.view)
		return out
	}, [branch.view, enabled, staged.view, turn.view, uncommitted.view, unstaged.view])
}
