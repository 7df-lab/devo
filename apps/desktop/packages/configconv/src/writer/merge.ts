/**
 * Deep merge utilities for combining Devo configs.
 *
 * When writing converted config to disk, we often need to merge with an existing
 * devo.json rather than overwrite it. This module implements merge strategies.
 */
import type { DevoConfig } from "../types/devo"

export type MergeStrategy = "preserve-existing" | "overwrite" | "merge"

/**
 * Deep merge two Devo configs according to the given strategy.
 *
 * @param existing - Current config on disk (target)
 * @param incoming - Converted config from cc2oc (source)
 * @param strategy - How to handle conflicts
 * @returns Merged config
 */
export function mergeConfigs(
	existing: Partial<DevoConfig>,
	incoming: Partial<DevoConfig>,
	strategy: MergeStrategy = "preserve-existing",
): Partial<DevoConfig> {
	if (strategy === "overwrite") {
		return { ...existing, ...incoming }
	}

	if (strategy === "preserve-existing") {
		return preserveMerge(existing, incoming)
	}

	// "merge" strategy -- deep merge with existing values taking precedence for scalars
	return deepMerge(existing, incoming)
}

/**
 * Preserve-existing merge: only add keys that don't already exist in the target.
 * For nested objects (mcp, agent, provider), merge at the key level.
 */
function preserveMerge(
	existing: Partial<DevoConfig>,
	incoming: Partial<DevoConfig>,
): Partial<DevoConfig> {
	const result = { ...existing }

	for (const [key, value] of Object.entries(incoming)) {
		const k = key as keyof DevoConfig
		const existingVal = result[k]

		if (existingVal === undefined) {
			// Key doesn't exist -- add it
			;(result as Record<string, unknown>)[key] = value
		} else if (isRecord(existingVal) && isRecord(value)) {
			// Both are objects -- merge at key level, preserving existing keys
			;(result as Record<string, unknown>)[key] = preserveMergeRecords(
				existingVal as Record<string, unknown>,
				value as Record<string, unknown>,
			)
		}
		// Scalar exists -- skip (preserve existing)
	}

	return result
}

/**
 * Deep merge: recursively merge objects, with existing values taking precedence.
 */
function deepMerge(
	existing: Partial<DevoConfig>,
	incoming: Partial<DevoConfig>,
): Partial<DevoConfig> {
	const result = { ...existing }

	for (const [key, value] of Object.entries(incoming)) {
		const existingVal = (result as Record<string, unknown>)[key]

		if (existingVal === undefined) {
			;(result as Record<string, unknown>)[key] = value
		} else if (isRecord(existingVal) && isRecord(value)) {
			;(result as Record<string, unknown>)[key] = deepMergeRecords(
				existingVal as Record<string, unknown>,
				value as Record<string, unknown>,
			)
		} else if (Array.isArray(existingVal) && Array.isArray(value)) {
			// Arrays: concat and deduplicate
			;(result as Record<string, unknown>)[key] = [...new Set([...existingVal, ...value])]
		}
		// Scalar exists -- keep existing
	}

	return result
}

function preserveMergeRecords(
	existing: Record<string, unknown>,
	incoming: Record<string, unknown>,
): Record<string, unknown> {
	const result = { ...existing }
	for (const [key, value] of Object.entries(incoming)) {
		if (!(key in result)) {
			result[key] = value
		}
	}
	return result
}

function deepMergeRecords(
	existing: Record<string, unknown>,
	incoming: Record<string, unknown>,
): Record<string, unknown> {
	const result = { ...existing }
	for (const [key, value] of Object.entries(incoming)) {
		const existingVal = result[key]
		if (existingVal === undefined) {
			result[key] = value
		} else if (isRecord(existingVal) && isRecord(value)) {
			result[key] = deepMergeRecords(
				existingVal as Record<string, unknown>,
				value as Record<string, unknown>,
			)
		} else if (Array.isArray(existingVal) && Array.isArray(value)) {
			result[key] = [...new Set([...existingVal, ...value])]
		}
		// Scalar exists -- keep existing
	}
	return result
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value)
}
