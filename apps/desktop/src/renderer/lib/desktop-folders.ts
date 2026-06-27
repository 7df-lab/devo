import type { DesktopFolder } from "../../preload/api"

export function normalizeDesktopFolderDirectory(directory: string): string {
	const trimmed = directory.trim()
	if (/^[a-zA-Z]:[\\/]?$/.test(trimmed)) return trimmed.endsWith("\\") ? trimmed : `${trimmed}\\`
	const normalized = trimmed.replace(/[\\/]+$/, "")
	return normalized || trimmed
}

export function desktopFolderNameFromDirectory(directory: string): string {
	const normalized = normalizeDesktopFolderDirectory(directory)
	if (!normalized) return "Folder"
	const parts = normalized.split(/[\\/]/).filter(Boolean)
	return parts.at(-1) ?? normalized
}

export function desktopFolderIdForDirectory(directory: string): string {
	const normalized = normalizeDesktopFolderDirectory(directory)
	let hash = 0
	for (let i = 0; i < normalized.length; i++) {
		hash = (hash * 31 + normalized.charCodeAt(i)) | 0
	}
	return `desktop-folder-${Math.abs(hash).toString(16).padStart(8, "0")}`
}

export function buildDesktopFolder(
	directory: string,
	name = desktopFolderNameFromDirectory(directory),
	addedAt = Date.now(),
): DesktopFolder {
	const normalizedDirectory = normalizeDesktopFolderDirectory(directory)
	return {
		id: desktopFolderIdForDirectory(normalizedDirectory),
		directory: normalizedDirectory,
		name,
		addedAt,
	}
}

export function desktopFolderProjectSlug(folder: DesktopFolder): string {
	const name = folder.name ?? desktopFolderNameFromDirectory(folder.directory)
	return `${name}-${folder.id.slice(0, 12)}`
}

export function upsertDesktopFolder(
	folders: readonly DesktopFolder[],
	folder: DesktopFolder,
): DesktopFolder[] {
	const normalizedDirectory = normalizeDesktopFolderDirectory(folder.directory)
	const normalizedFolder = { ...folder, directory: normalizedDirectory }
	const existingIndex = folders.findIndex(
		(existing) => normalizeDesktopFolderDirectory(existing.directory) === normalizedDirectory,
	)
	if (existingIndex === -1) return [...folders, normalizedFolder]

	const next = [...folders]
	next[existingIndex] = {
		...next[existingIndex],
		...normalizedFolder,
		addedAt: next[existingIndex].addedAt,
	}
	return next
}

export function removeDesktopFolder(
	folders: readonly DesktopFolder[],
	directory: string,
): DesktopFolder[] {
	const normalizedDirectory = normalizeDesktopFolderDirectory(directory)
	return folders.filter(
		(folder) => normalizeDesktopFolderDirectory(folder.directory) !== normalizedDirectory,
	)
}
