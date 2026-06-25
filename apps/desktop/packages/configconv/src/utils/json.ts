/**
 * JSONC (JSON with comments) reading utilities.
 */
import * as jsonc from "jsonc-parser"

/**
 * Parse a JSONC string (supports comments and trailing commas).
 */
export function parseJsonc<T = unknown>(content: string): T {
	const errors: jsonc.ParseError[] = []
	const result = jsonc.parse(content, errors, {
		allowTrailingComma: true,
		disallowComments: false,
	})

	if (errors.length > 0) {
		const firstError = errors[0]
		throw new Error(
			`JSONC parse error at offset ${firstError.offset}: ${jsonc.printParseErrorCode(firstError.error)}`,
		)
	}

	return result as T
}

/**
 * Stringify an object to pretty JSON (not JSONC -- we don't add comments).
 */
export function stringifyJson(value: unknown, indent: string = "\t"): string {
	return `${JSON.stringify(value, null, indent)}\n`
}
