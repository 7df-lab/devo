/**
 * Helpers for custom provider ID collisions with built-in templates.
 */

/** Returns true when a new custom provider id collides with a built-in template. */
export function isBuiltinTemplateProviderId(
	providerId: string,
	templateIds: ReadonlySet<string>,
): boolean {
	const id = providerId.trim()
	return id.length > 0 && templateIds.has(id)
}

/** Suggests an alternate id that does not collide with known templates. */
export function suggestNonTemplateProviderId(
	providerId: string,
	templateIds: ReadonlySet<string>,
): string {
	const base = providerId.trim() || "provider"
	let candidate = `${base}-custom`
	let suffix = 2
	while (templateIds.has(candidate)) {
		candidate = `${base}-custom-${suffix}`
		suffix += 1
	}
	return candidate
}
