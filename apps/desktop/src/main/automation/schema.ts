/**
 * Drizzle ORM schema for automation tables.
 *
 * These tables store scheduling state and run history.
 * Automation config lives on disk as JSON + prompt.md files;
 * SQLite only holds timing and execution state.
 */

import { index, int, sqliteTable, text } from "drizzle-orm/sqlite-core"

export const automations = sqliteTable("automations", {
	id: text("id").primaryKey(),
	nextRunAt: int("next_run_at"),
	lastRunAt: int("last_run_at"),
	runCount: int("run_count").notNull().default(0),
	consecutiveFailures: int("consecutive_failures").notNull().default(0),
	createdAt: int("created_at").notNull(),
	updatedAt: int("updated_at").notNull(),
})

export const automationRuns = sqliteTable(
	"automation_runs",
	{
		id: text("id").primaryKey(),
		automationId: text("automation_id")
			.notNull()
			.references(() => automations.id, { onDelete: "cascade" }),
		workspace: text("workspace").notNull(),
		status: text("status").notNull(),
		attempt: int("attempt").notNull().default(1),
		sessionId: text("session_id"),
		worktreePath: text("worktree_path"),
		startedAt: int("started_at"),
		completedAt: int("completed_at"),
		timeoutAt: int("timeout_at"),
		resultTitle: text("result_title"),
		resultSummary: text("result_summary"),
		resultHasActionable: int("result_has_actionable", { mode: "boolean" }),
		resultBranch: text("result_branch"),
		resultPrUrl: text("result_pr_url"),
		errorMessage: text("error_message"),
		archivedReason: text("archived_reason"),
		archivedAssistantMessage: text("archived_assistant_message"),
		readAt: int("read_at"),
		createdAt: int("created_at").notNull(),
		updatedAt: int("updated_at").notNull(),
	},
	(table) => [
		index("idx_runs_automation").on(table.automationId),
		index("idx_runs_status").on(table.status),
		index("idx_runs_created").on(table.createdAt),
	],
)
