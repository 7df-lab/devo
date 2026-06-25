/**
 * Skills compatibility verifier.
 *
 * Skills use the same SKILL.md format in both tools, so this mostly
 * validates they parse correctly and reports on compatibility.
 */

import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import type { SkillInfo } from "../types/scan-result"

export interface SkillsVerificationResult {
	/** Skills that are fully compatible */
	compatible: SkillInfo[]
	/** Skills that need attention */
	needsAttention: SkillInfo[]
	report: MigrationReport
}

/**
 * Verify skills are compatible with Devo.
 * Since both tools use the same SKILL.md format, this is mostly validation.
 */
export function verifySkills(skills: SkillInfo[]): SkillsVerificationResult {
	const compatible: SkillInfo[] = []
	const needsAttention: SkillInfo[] = []
	const report = createEmptyReport()

	for (const skill of skills) {
		if (!skill.name) {
			needsAttention.push(skill)
			report.warnings.push(`Skill at "${skill.path}" has no name in frontmatter.`)
			continue
		}

		if (!skill.description) {
			needsAttention.push(skill)
			report.warnings.push(
				`Skill "${skill.name}" at "${skill.path}" has no description in frontmatter. ` +
					`Devo may not load it correctly.`,
			)
			continue
		}

		compatible.push(skill)
		report.migrated.push({
			category: "skills",
			source: skill.path,
			target: `(compatible, no changes needed)`,
			details: skill.isSymlink ? `Symlink -> ${skill.symlinkTarget}` : "Direct file",
		})
	}

	if (compatible.length > 0) {
		report.migrated.push({
			category: "skills",
			source: `${compatible.length} skills`,
			target: "Devo reads .claude/skills/ natively",
			details: "No migration needed -- Devo has built-in Claude Code skill compatibility",
		})
	}

	return { compatible, needsAttention, report }
}
