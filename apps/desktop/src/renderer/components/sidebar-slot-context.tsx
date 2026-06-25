import { createContext, type ReactNode, useCallback, useContext, useState } from "react"

interface SidebarSlotContextValue {
	/** Content to render in the sidebar body (null = use default) */
	content: ReactNode | null
	/** Content to render in the sidebar footer (null = use default, false = hide) */
	footer: ReactNode | null | false
	/** Set the sidebar body content (call from child routes) */
	setContent: (content: ReactNode | null) => void
	/** Set the sidebar footer content (null = default, false = hide) */
	setFooter: (footer: ReactNode | null | false) => void
}

const SidebarSlotContext = createContext<SidebarSlotContextValue | null>(null)

export function SidebarSlotProvider({ children }: { children: ReactNode }) {
	const [content, setContentState] = useState<ReactNode | null>(null)
	const [footer, setFooterState] = useState<ReactNode | null | false>(null)

	const setContent = useCallback((c: ReactNode | null) => {
		setContentState(c)
	}, [])

	const setFooter = useCallback((f: ReactNode | null | false) => {
		setFooterState(f)
	}, [])

	return (
		<SidebarSlotContext.Provider value={{ content, footer, setContent, setFooter }}>
			{children}
		</SidebarSlotContext.Provider>
	)
}

/**
 * Hook for the layout to read the current sidebar slot content and footer.
 */
export function useSidebarSlot(): { content: ReactNode | null; footer: ReactNode | null | false } {
	const ctx = useContext(SidebarSlotContext)
	if (!ctx) throw new Error("useSidebarSlot must be used within SidebarSlotProvider")
	return { content: ctx.content, footer: ctx.footer }
}

/**
 * Hook for child routes to set the sidebar content and footer.
 */
export function useSetSidebarSlot() {
	const ctx = useContext(SidebarSlotContext)
	if (!ctx) throw new Error("useSetSidebarSlot must be used within SidebarSlotProvider")
	return { setContent: ctx.setContent, setFooter: ctx.setFooter }
}
