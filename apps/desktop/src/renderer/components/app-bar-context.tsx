import { createContext, type ReactNode, useCallback, useContext, useState } from "react"

interface AppBarContextValue {
	/** Content to render in the right section of the app bar */
	content: ReactNode | null
	/** Set the right-section content (call from child pages) */
	setContent: (content: ReactNode | null) => void
}

const AppBarContext = createContext<AppBarContextValue | null>(null)

export function AppBarProvider({ children }: { children: ReactNode }) {
	const [content, setContentState] = useState<ReactNode | null>(null)

	const setContent = useCallback((c: ReactNode | null) => {
		setContentState(c)
	}, [])

	return <AppBarContext.Provider value={{ content, setContent }}>{children}</AppBarContext.Provider>
}

/**
 * Hook for the AppBar to read the current page-specific content.
 */
export function useAppBarContent() {
	const ctx = useContext(AppBarContext)
	if (!ctx) throw new Error("useAppBarContent must be used within AppBarProvider")
	return ctx.content
}

/**
 * Hook for child pages to set their content in the app bar.
 */
export function useSetAppBarContent() {
	const ctx = useContext(AppBarContext)
	if (!ctx) throw new Error("useSetAppBarContent must be used within AppBarProvider")
	return ctx.setContent
}
