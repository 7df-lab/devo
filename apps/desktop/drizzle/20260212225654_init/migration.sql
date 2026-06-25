CREATE TABLE `automation_runs` (
	`id` text PRIMARY KEY,
	`automation_id` text NOT NULL,
	`workspace` text NOT NULL,
	`status` text NOT NULL,
	`attempt` integer DEFAULT 1 NOT NULL,
	`session_id` text,
	`started_at` integer,
	`completed_at` integer,
	`timeout_at` integer,
	`result_title` text,
	`result_summary` text,
	`result_has_actionable` integer,
	`result_branch` text,
	`result_pr_url` text,
	`error_message` text,
	`archived_reason` text,
	`archived_assistant_message` text,
	`read_at` integer,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	CONSTRAINT `fk_automation_runs_automation_id_automations_id_fk` FOREIGN KEY (`automation_id`) REFERENCES `automations`(`id`) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE TABLE `automations` (
	`id` text PRIMARY KEY,
	`next_run_at` integer,
	`last_run_at` integer,
	`run_count` integer DEFAULT 0 NOT NULL,
	`consecutive_failures` integer DEFAULT 0 NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL
);
--> statement-breakpoint
CREATE INDEX `idx_runs_automation` ON `automation_runs` (`automation_id`);--> statement-breakpoint
CREATE INDEX `idx_runs_status` ON `automation_runs` (`status`);--> statement-breakpoint
CREATE INDEX `idx_runs_created` ON `automation_runs` (`created_at`);