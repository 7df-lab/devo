import { mkdir, stat } from "node:fs/promises"
import path from "node:path"
import type {
	CreateDesktopFolderInput,
	CreateDesktopFolderResult,
	DesktopFolderStat,
	DesktopFolderStatus,
} from "../preload/api"

function isMissingPathError(error: unknown): boolean {
	return (
		typeof error === "object" &&
		error !== null &&
		"code" in error &&
		(error as NodeJS.ErrnoException).code === "ENOENT"
	)
}

export function normalizeDesktopFolderDirectory(directory: string): string {
	const resolved = path.resolve(directory)
	const root = path.parse(resolved).root
	if (resolved === root) return resolved
	return resolved.replace(/[\\/]+$/, "")
}

async function statDesktopFolder(directory: string): Promise<DesktopFolderStat> {
	const normalized = normalizeDesktopFolderDirectory(directory)
	try {
		const info = await stat(normalized)
		const status: DesktopFolderStatus = info.isDirectory() ? "available" : "not_directory"
		return { directory: normalized, status }
	} catch (error) {
		if (isMissingPathError(error)) {
			return { directory: normalized, status: "missing" }
		}
		throw error
	}
}

export async function statDesktopFolders(directories: string[]): Promise<DesktopFolderStat[]> {
	return await Promise.all(directories.map(statDesktopFolder))
}

function validateFolderName(name: string): string {
	const trimmed = name.trim()
	if (!trimmed) {
		throw new Error("Folder name is required")
	}
	if (
		trimmed === "." ||
		trimmed === ".." ||
		trimmed.includes("/") ||
		trimmed.includes("\\") ||
		path.basename(trimmed) !== trimmed
	) {
		throw new Error("Folder name cannot contain path separators")
	}
	return trimmed
}

export async function createDesktopFolder(
	input: CreateDesktopFolderInput,
): Promise<CreateDesktopFolderResult> {
	const name = validateFolderName(input.name)
	const parentDirectory = normalizeDesktopFolderDirectory(input.parentDirectory)
	const parent = await statDesktopFolder(parentDirectory)
	if (parent.status === "missing") {
		throw new Error("Parent folder does not exist")
	}
	if (parent.status === "not_directory") {
		throw new Error("Parent path is not a directory")
	}

	const directory = normalizeDesktopFolderDirectory(path.join(parentDirectory, name))
	const existing = await statDesktopFolder(directory)
	if (existing.status === "available") {
		throw new Error("Folder already exists")
	}
	if (existing.status === "not_directory") {
		throw new Error("Path already exists and is not a directory")
	}

	await mkdir(directory)
	return { directory, name }
}
