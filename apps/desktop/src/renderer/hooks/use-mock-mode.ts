/**
 * Hook that manages mock mode lifecycle.
 *
 * When mock mode activates: hydrates all Jotai atoms with fixture data,
 * marks discovery as loaded, and fakes the server connection.
 *
 * When mock mode deactivates: clears mock atoms and resets discovery
 * so the real discovery flow can run.
 */
import { useAtomValue } from "jotai"
import { useEffect, useRef } from "react"
import { serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { discoveryAtom } from "../atoms/discovery"
import { messagesFamily } from "../atoms/messages"
import { isMockModeAtom } from "../atoms/mock-mode"
import { partsFamily, partStorageKey } from "../atoms/parts"
import { sessionFamily, sessionIdsAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { sessionDiffFamily } from "../atoms/ui"
import { createLogger } from "../lib/logger"
import {
	MOCK_DIFFS,
	MOCK_DISCOVERY,
	MOCK_MESSAGES,
	MOCK_PARTS,
	MOCK_SESSION_ENTRIES,
	MOCK_SESSION_IDS,
} from "../lib/mock-data"
import { disconnect } from "../services/connection-manager"
import { resetDiscoveryGuard } from "./use-discovery"

const log = createLogger("mock-mode")

/**
 * Call from the root layout. Watches `isMockModeAtom` and hydrates/clears
 * Jotai atoms accordingly. Returns the current mock mode state.
 */
export function useMockMode(): boolean {
	const isMockMode = useAtomValue(isMockModeAtom)
	const prevRef = useRef(false)

	useEffect(() => {
		const wasActive = prevRef.current
		prevRef.current = isMockMode

		if (isMockMode && !wasActive) {
			activateMockMode()
		} else if (!isMockMode && wasActive) {
			deactivateMockMode()
		}
	}, [isMockMode])

	return isMockMode
}

// ============================================================
// Activation
// ============================================================

function activateMockMode(): void {
	log.info("Activating mock mode")

	// Disconnect from real server if connected
	disconnect()

	// 1. Hydrate discovery (marks loaded=true so useDiscovery() no-ops)
	appStore.set(discoveryAtom, MOCK_DISCOVERY)

	// 2. Hydrate sessions
	appStore.set(sessionIdsAtom, new Set(MOCK_SESSION_IDS))
	for (const [sessionId, entry] of MOCK_SESSION_ENTRIES) {
		appStore.set(sessionFamily(sessionId), entry)
	}

	// 3. Hydrate messages and parts
	for (const [sessionId, messages] of MOCK_MESSAGES) {
		appStore.set(messagesFamily(sessionId), messages)
	}
	for (const [sessionId, sessionParts] of MOCK_PARTS) {
		for (const [messageId, parts] of Object.entries(sessionParts)) {
			appStore.set(partsFamily(partStorageKey(sessionId, messageId)), parts)
			appStore.set(partsFamily(messageId), parts)
		}
	}

	// 4. Hydrate diffs
	for (const [sessionId, diffs] of MOCK_DIFFS) {
		appStore.set(sessionDiffFamily(sessionId), diffs)
	}

	// 5. Fake server connection state
	appStore.set(serverUrlAtom, "http://mock-server:3100")
	appStore.set(serverConnectedAtom, true)

	log.info("Mock mode activated", {
		sessions: MOCK_SESSION_IDS.size,
		messages: MOCK_MESSAGES.size,
	})
}

// ============================================================
// Deactivation
// ============================================================

function deactivateMockMode(): void {
	log.info("Deactivating mock mode")

	// 1. Clear session atoms
	for (const sessionId of MOCK_SESSION_IDS) {
		appStore.set(sessionFamily(sessionId), null)
	}
	appStore.set(sessionIdsAtom, new Set<string>())

	// 2. Clear message and part atoms
	for (const [sessionId, messages] of MOCK_MESSAGES) {
		appStore.set(messagesFamily(sessionId), [])
		for (const msg of messages) {
			appStore.set(partsFamily(partStorageKey(sessionId, msg.id)), [])
			appStore.set(partsFamily(msg.id), [])
		}
	}

	// 3. Clear diff atoms
	for (const sessionId of MOCK_SESSION_IDS) {
		appStore.set(sessionDiffFamily(sessionId), [])
	}

	// 4. Reset discovery so useDiscovery() will re-run
	appStore.set(discoveryAtom, {
		loaded: false,
		loading: false,
		error: null,
		phase: "idle",
		projects: [],
	})

	// 5. Reset connection state
	appStore.set(serverUrlAtom, null)
	appStore.set(serverConnectedAtom, false)

	// 6. Reset discovery guard so the real discovery flow can re-run
	resetDiscoveryGuard()

	log.info("Mock mode deactivated, real discovery will restart")
}
