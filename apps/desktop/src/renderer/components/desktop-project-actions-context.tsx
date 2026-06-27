import { createContext, useContext, type ReactNode } from "react"
import type { DesktopFolder } from "../../preload/api"

interface DesktopProjectActionsContextValue {
	startFromScratch?: () => void | Promise<void>
	useExistingFolder?: () => Promise<DesktopFolder | null>
}

const DesktopProjectActionsContext = createContext<DesktopProjectActionsContextValue>({})

export function DesktopProjectActionsProvider({
	children,
	startFromScratch,
	useExistingFolder,
}: {
	children: ReactNode
	startFromScratch?: () => void | Promise<void>
	useExistingFolder?: () => Promise<DesktopFolder | null>
}) {
	return (
		<DesktopProjectActionsContext.Provider value={{ startFromScratch, useExistingFolder }}>
			{children}
		</DesktopProjectActionsContext.Provider>
	)
}

export function useDesktopProjectActions(): DesktopProjectActionsContextValue {
	return useContext(DesktopProjectActionsContext)
}
