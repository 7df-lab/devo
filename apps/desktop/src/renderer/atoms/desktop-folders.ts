import { atom } from "jotai"
import type { DesktopFolder, DesktopFolderStatus } from "../../preload/api"

export const desktopFoldersAtom = atom<DesktopFolder[]>([])

export const desktopFolderStatusByDirectoryAtom = atom<Record<string, DesktopFolderStatus>>({})
