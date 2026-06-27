import { atomWithStorage } from "jotai/utils"
import {
	DEFAULT_SIDEBAR_PREFERENCES,
	type SidebarPreferences,
} from "./sidebar-data"

export const sidebarPreferencesAtom = atomWithStorage<SidebarPreferences>(
	"devo:sidebarPreferences",
	DEFAULT_SIDEBAR_PREFERENCES,
)
