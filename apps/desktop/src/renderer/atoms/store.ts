import { createStore } from "jotai"

/**
 * Single Jotai store instance, used both in React (via Provider)
 * and imperatively (via store.get/set) in services like connection-manager.
 */
export const appStore = createStore()

// Convenience re-exports for imperative usage
export const getAtom = appStore.get.bind(appStore)
export const setAtom = appStore.set.bind(appStore)
export const subAtom = appStore.sub.bind(appStore)
