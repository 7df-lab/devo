/**
 * Types for migration reports.
 */
import type { MigrationCategory } from "./conversion-result"

export interface MigrationReport {
	/** Successfully migrated items */
	migrated: MigrationItem[]
	/** Items that were skipped */
	skipped: MigrationItem[]
	/** Non-fatal warnings */
	warnings: string[]
	/** Actions the user must take manually */
	manualActions: string[]
	/** Errors encountered */
	errors: string[]
}

export interface MigrationItem {
	/** Migration category */
	category: MigrationCategory
	/** Source description (CC file path or description) */
	source: string
	/** Target description (OC file path or description) */
	target: string
	/** Additional details */
	details?: string
}

export function createEmptyReport(): MigrationReport {
	return {
		migrated: [],
		skipped: [],
		warnings: [],
		manualActions: [],
		errors: [],
	}
}

export function mergeReports(...reports: MigrationReport[]): MigrationReport {
	return {
		migrated: reports.flatMap((r) => r.migrated),
		skipped: reports.flatMap((r) => r.skipped),
		warnings: reports.flatMap((r) => r.warnings),
		manualActions: reports.flatMap((r) => r.manualActions),
		errors: reports.flatMap((r) => r.errors),
	}
}
