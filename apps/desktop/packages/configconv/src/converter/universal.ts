/**
 * Universal converter orchestrator.
 *
 * Converts between any supported agent configuration formats:
 *   Claude Code <-> Devo <-> Cursor
 *
 * Architecture:
 *   Source format -> toCanonical() -> Canonical IR -> fromCanonical() -> Target format
 */
import type {
	AgentFormat,
	CanonicalConversionResult,
	CanonicalScanResult,
	UniversalConvertOptions,
} from "../types/canonical"
import type { CursorScanResult } from "../types/cursor"
import type { ScanResult } from "../types/scan-result"
import { canonicalToClaudeCode } from "./from-canonical/to-claude-code"
import { canonicalToCursor } from "./from-canonical/to-cursor"
import { canonicalToDevo } from "./from-canonical/to-devo"
import { claudeCodeToCanonical } from "./to-canonical/claude-code"
import { cursorToCanonical } from "./to-canonical/cursor"
import type { DevoScanResult } from "./to-canonical/devo"
import { openCodeToCanonical } from "./to-canonical/devo"

// ============================================================
// Type-safe scan result union
// ============================================================

export type AnyScanResult =
	| { format: "claude-code"; data: ScanResult }
	| { format: "devo"; data: DevoScanResult }
	| { format: "cursor"; data: CursorScanResult }

// ============================================================
// Universal convert
// ============================================================

/**
 * Convert from any supported format to any other supported format.
 *
 * @param scan - The scanned configuration data (with format tag)
 * @param options - Conversion options
 * @returns Conversion result with files to write and a migration report
 */
export function universalConvert(
	scan: AnyScanResult,
	options: Omit<UniversalConvertOptions, "from"> & { to: AgentFormat },
): CanonicalConversionResult {
	// Step 1: Convert to canonical intermediate representation
	const canonical = toCanonical(scan)

	// Step 2: Convert from canonical to target format
	return fromCanonical(canonical, options.to, options)
}

/**
 * Convert any format-specific scan result to canonical form.
 */
export function toCanonical(scan: AnyScanResult): CanonicalScanResult {
	switch (scan.format) {
		case "claude-code":
			return claudeCodeToCanonical(scan.data)
		case "devo":
			return openCodeToCanonical(scan.data)
		case "cursor":
			return cursorToCanonical(scan.data)
	}
}

/**
 * Convert canonical form to a specific target format.
 */
export function fromCanonical(
	canonical: CanonicalScanResult,
	targetFormat: AgentFormat,
	options?: {
		modelOverrides?: Record<string, string>
		defaultModel?: string
		defaultSmallModel?: string
	},
): CanonicalConversionResult {
	switch (targetFormat) {
		case "claude-code":
			return canonicalToClaudeCode(canonical)
		case "devo":
			return canonicalToDevo(canonical, options)
		case "cursor":
			return canonicalToCursor(canonical)
	}
}

/**
 * Get all supported format pairs for conversion.
 */
export function getSupportedConversions(): Array<{ from: AgentFormat; to: AgentFormat }> {
	const formats: AgentFormat[] = ["claude-code", "devo", "cursor"]
	const pairs: Array<{ from: AgentFormat; to: AgentFormat }> = []

	for (const from of formats) {
		for (const to of formats) {
			if (from !== to) {
				pairs.push({ from, to })
			}
		}
	}

	return pairs
}

/**
 * Get human-readable format name.
 */
export function formatName(format: AgentFormat): string {
	switch (format) {
		case "claude-code":
			return "Claude Code"
		case "devo":
			return "Devo"
		case "cursor":
			return "Cursor"
	}
}
