/**
 * YAML frontmatter parsing utilities.
 * Handles the lenient YAML that Claude Code allows (unquoted colons, etc.).
 */
import YAML from "yaml"

interface FrontmatterResult {
	frontmatter: Record<string, unknown>
	body: string
}

const FRONTMATTER_RE = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/

/**
 * Parse a markdown file with YAML frontmatter.
 * Falls back to lenient parsing if strict YAML fails (Claude Code compat).
 */
export function parseFrontmatter(content: string): FrontmatterResult {
	const match = content.match(FRONTMATTER_RE)
	if (!match) {
		return { frontmatter: {}, body: content.trim() }
	}

	const rawYaml = match[1]
	const body = (match[2] ?? "").trim()

	try {
		const frontmatter = YAML.parse(rawYaml)
		if (typeof frontmatter !== "object" || frontmatter === null) {
			return { frontmatter: {}, body }
		}
		return { frontmatter: frontmatter as Record<string, unknown>, body }
	} catch {
		// Claude Code allows invalid YAML -- try fallback sanitization
		return { frontmatter: fallbackParseFrontmatter(rawYaml), body }
	}
}

/**
 * Fallback YAML parser that handles common Claude Code quirks:
 * - Unquoted values containing colons
 * - Tool lists without proper array syntax
 */
function fallbackParseFrontmatter(raw: string): Record<string, unknown> {
	const result: Record<string, unknown> = {}

	for (const line of raw.split("\n")) {
		const trimmed = line.trim()
		if (!trimmed || trimmed.startsWith("#")) continue

		const colonIdx = trimmed.indexOf(":")
		if (colonIdx === -1) continue

		const key = trimmed.slice(0, colonIdx).trim()
		const value = trimmed.slice(colonIdx + 1).trim()

		if (!key) continue

		// Try to parse value
		if (value === "" || value === "~" || value === "null") {
			result[key] = null
		} else if (value === "true") {
			result[key] = true
		} else if (value === "false") {
			result[key] = false
		} else if (/^-?\d+$/.test(value)) {
			result[key] = parseInt(value, 10)
		} else if (/^-?\d+\.\d+$/.test(value)) {
			result[key] = parseFloat(value)
		} else {
			result[key] = value
		}
	}

	return result
}

/**
 * Serialize frontmatter + body back to a markdown file.
 */
export function serializeFrontmatter(frontmatter: Record<string, unknown>, body: string): string {
	const yamlStr = YAML.stringify(frontmatter, {
		indent: 2,
		lineWidth: 0,
		defaultStringType: "QUOTE_DOUBLE",
		defaultKeyType: "PLAIN",
	}).trim()

	return `---\n${yamlStr}\n---\n${body ? `\n${body}\n` : ""}`
}
