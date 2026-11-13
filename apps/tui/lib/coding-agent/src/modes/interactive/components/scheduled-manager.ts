import { type Component, type Focusable, getKeybindings, truncateToWidth } from "@earendil-works/pi-tui";
import type { AgentScheduledManagementAction } from "../../../core/cron-jobs.js";
import type { AgentConnectionScheduledTask } from "../../agent-connection/types.js";
import { keyHint } from "./keybinding-hints.js";
import { theme } from "../theme/theme.js";
import { shouldTreatAsBack } from "./modal-back.js";

type ScheduledManagerMode = { type: "list" } | { type: "actions"; scheduledId: string; selectedIndex: number };

export interface ScheduledManagerActionState {
	pending?: string;
	error?: string;
}

export interface ScheduledManagerOptions {
	/** Shared across reopened panels; closing does not cancel an accepted operation. */
	actionState?: ScheduledManagerActionState;
	getScheduledTasks: () => readonly AgentConnectionScheduledTask[];
	getRows: () => number;
	onAction: (scheduled: AgentConnectionScheduledTask, action: AgentScheduledManagementAction) => Promise<void>;
	onClose: () => void;
	requestRender: () => void;
}

export class ScheduledManagerComponent implements Component, Focusable {
	private selectedScheduledId: string | undefined;
	private mode: ScheduledManagerMode = { type: "list" };
	private readonly actionState: ScheduledManagerActionState;
	private _focused = false;

	constructor(private readonly options: ScheduledManagerOptions) {
		this.actionState = options.actionState ?? {};
	}

	get focused(): boolean {
		return this._focused;
	}

	set focused(value: boolean) {
		this._focused = value;
	}

	invalidate(): void {}

	private get scheduledTasks(): AgentConnectionScheduledTask[] {
		return [...this.options.getScheduledTasks()].sort((left, right) => {
			const sessionOrder = this.sessionLabel(left).localeCompare(this.sessionLabel(right));
			if (sessionOrder !== 0) return sessionOrder;
			if (left.job.source !== right.job.source) return left.job.source === "heartbeat" ? -1 : 1;
			return left.job.createdAt.localeCompare(right.job.createdAt);
		});
	}

	handleInput(data: string): void {
		const keybindings = getKeybindings();
		if (keybindings.matches(data, "tui.select.cancel") || keybindings.matches(data, "app.heartbeats.open")) {
			this.options.onClose();
			return;
		}
		if (shouldTreatAsBack(data)) {
			if (this.actionState.pending) { this.options.onClose(); return; }
			if (this.mode.type === "list") {
				this.options.onClose();
			} else {
				this.mode = { type: "list" };
				this.actionState.error = undefined;
				this.options.requestRender();
			}
			return;
		}
		if (this.actionState.pending) return;
		if (keybindings.matches(data, "tui.select.up")) {
			this.moveSelection(-1);
			return;
		}
		if (keybindings.matches(data, "tui.select.down")) {
			this.moveSelection(1);
			return;
		}
		if (this.mode.type === "list" && keybindings.matches(data, "app.heartbeats.openSelected")) {
			void this.confirmSelection();
			return;
		}
		if (keybindings.matches(data, "tui.select.confirm")) {
			void this.confirmSelection();
		}
	}

	render(width: number): string[] {
		const scheduledTasks = this.scheduledTasks;
		if (!scheduledTasks.some((scheduled) => scheduled.job.id === this.selectedScheduledId)) {
			this.selectedScheduledId = scheduledTasks[0]?.job.id;
		}
		if (this.mode.type !== "list") {
			const scheduledId = this.mode.scheduledId;
			if (!scheduledTasks.some((scheduled) => scheduled.job.id === scheduledId)) this.mode = { type: "list" };
		}
		if (width < 1) return [];
		const rows = Math.max(1, this.options.getRows());
		const feedback = this.actionState.pending
			? theme.fg("accent", `${this.actionState.pending}…`)
			: this.actionState.error ? theme.fg("error", `Error: ${this.actionState.error}`) : undefined;
		const selectedTask = this.mode.type === "actions" ? this.findScheduled(this.mode.scheduledId) : undefined;
		const title = selectedTask ? `Scheduled · ${this.singleLine(selectedTask.job.label || selectedTask.job.prompt || this.defaultScheduledName(selectedTask))}` : `Scheduled · ${scheduledTasks.length} tasks`;
		const lines: string[] = [theme.fg("accent", title)];
		const footer = this.mode.type === "list"
			? `${keyHint("app.modal.back", "back")} ${keyHint("tui.select.cancel", "close", { primaryOnly: true })} ${keyHint("tui.select.confirm", "details", { primaryOnly: true })}`
			: `${keyHint("app.modal.back", "back")} ${keyHint("tui.select.cancel", "close", { primaryOnly: true })} ${keyHint("tui.select.confirm", "confirm", { primaryOnly: true })}`;
		const capacity = Math.max(1, rows - 2 - (feedback ? 1 : 0) - (this.actionState.pending ? 1 : 0));
		if (this.mode.type === "list") {
			const selected = this.getSelectedIndex(scheduledTasks);
			const start = Math.max(0, Math.min(selected - Math.floor(capacity / 2), scheduledTasks.length - capacity));
			for (let index = start; index < Math.min(start + capacity, scheduledTasks.length); index++) {
				const task = scheduledTasks[index]!;
				lines.push(`${index === selected ? "›" : " "} ${task.job.lastError ? theme.fg("error", "error") : this.formatStatus(task)} · ${this.singleLine(task.job.label || task.job.prompt || this.defaultScheduledName(task))}`);
			}
			if (!scheduledTasks.length) lines.push(theme.fg("muted", "No running or paused scheduled tasks"));
			if (scheduledTasks.length > capacity) lines[0] += ` · ${selected + 1}/${scheduledTasks.length}`;
		} else {
			const task = this.findScheduled(this.mode.scheduledId)!;
			const actions = this.availableActions(task);
			const details = [this.formatScheduledDetails(task), `Next: ${task.job.nextRunAt ? this.formatTimestamp(task.job.nextRunAt) : "—"}`,
				...(task.job.lastError ? [`Last error: ${this.singleLine(task.job.lastError)}`] : [])];
			// Preserve selected actions and navigation even in a very short terminal.
			const actionRows = Math.min(actions.length, capacity);
			lines.push(...details.slice(Math.max(0, details.length - Math.max(0, capacity - actionRows))));
			const start = actionRows < actions.length ? this.mode.selectedIndex : 0;
			for (let index = start; index < Math.min(start + actionRows, actions.length); index++) {
				lines.push(`${index === this.mode.selectedIndex ? "›" : " "} ${actions[index]!.label}`);
			}
		}
		if (feedback) lines.push(feedback);
		if (this.actionState.pending) lines.push(theme.fg("muted", "Closing does not cancel this operation."));
		lines.push(theme.fg("muted", footer));
		// Reserve the final navigation line instead of clipping it off at the overlay boundary.
		const bounded = lines.length > rows ? [...lines.slice(0, Math.max(0, rows - 1)), lines[lines.length - 1]!] : lines;
		return bounded.map((line) => truncateToWidth(line, width));
	}

	private moveSelection(delta: number): void {
		if (this.mode.type === "list") {
			const scheduledTasks = this.scheduledTasks;
			if (scheduledTasks.length === 0) return;
			const selectedIndex = this.getSelectedIndex(scheduledTasks);
			const nextIndex = Math.max(0, Math.min(selectedIndex + delta, scheduledTasks.length - 1));
			this.selectedScheduledId = scheduledTasks[nextIndex]?.job.id;
		} else {
			const count = this.availableActions(this.findScheduled(this.mode.scheduledId)).length;
			this.mode = { ...this.mode, selectedIndex: Math.max(0, Math.min(this.mode.selectedIndex + delta, count - 1)) };
		}
		this.options.requestRender();
	}

	private async confirmSelection(): Promise<void> {
		// Do not submit an action when the terminal cannot display identity, selection and navigation.
		if (this.mode.type === "actions" && this.options.getRows() < 3) return;
		if (this.mode.type === "list") {
			const scheduledTasks = this.scheduledTasks;
			const scheduled = scheduledTasks[this.getSelectedIndex(scheduledTasks)];
			if (scheduled) {
				this.mode = { type: "actions", scheduledId: scheduled.job.id, selectedIndex: 0 };
				this.options.requestRender();
			}
			return;
		}
		const scheduled = this.findScheduled(this.mode.scheduledId);
		if (!scheduled) {
			this.mode = { type: "list" };
			this.options.requestRender();
			return;
		}
		const selected = this.availableActions(scheduled)[this.mode.selectedIndex];
		if (!selected) return;
		await this.runAction(scheduled, selected.action);
	}

	private async runAction(scheduled: AgentConnectionScheduledTask, action: AgentScheduledManagementAction): Promise<void> {
		if (this.actionState.pending) return;
		const label = action === "pause" ? "Pausing scheduled task" : action === "resume" ? "Resuming scheduled task" : "Stopping scheduled task";
		this.actionState.pending = label;
		this.actionState.error = undefined;
		this.options.requestRender();
		try {
			await this.options.onAction(scheduled, action);
			this.mode = { type: "list" };
		} catch (error) {
			this.actionState.error = `${label} failed: ${error instanceof Error ? error.message : String(error)}`;
		} finally {
			this.actionState.pending = undefined;
			this.options.requestRender();
		}
	}

	private availableActions(
		scheduled: AgentConnectionScheduledTask | undefined,
	): Array<{ label: string; action: AgentScheduledManagementAction }> {
		if (!scheduled) return [];
		return [
			scheduled.job.status === "paused"
				? { label: "Resume scheduled task", action: "resume" }
				: { label: "Pause scheduled task", action: "pause" },
			{ label: "Stop scheduled task", action: "stop" },
		];
	}

	private getSelectedIndex(scheduledTasks: readonly AgentConnectionScheduledTask[]): number {
		const index = scheduledTasks.findIndex((scheduled) => scheduled.job.id === this.selectedScheduledId);
		return index < 0 ? 0 : index;
	}

	private findScheduled(id: string): AgentConnectionScheduledTask | undefined {
		return this.scheduledTasks.find((scheduled) => scheduled.job.id === id);
	}

	private sessionLabel(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.sessionName?.trim() || this.singleLine(scheduled.firstMessage ?? "") || scheduled.job.sessionId;
	}

	private formatStatus(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.status === "active" ? theme.fg("success", "active") : theme.fg("warning", "paused");
	}

	private formatScheduledDetails(scheduled: AgentConnectionScheduledTask): string {
		const delivery = scheduled.job.deliveryMode === "follow_up" ? "follow-up" : "steer";
		return `${this.sourceLabel(scheduled)} · ${this.sessionLabel(scheduled)} · ${scheduled.job.status} · ${scheduled.job.schedule.expression} · ${delivery} · ${scheduled.job.runCount} runs`;
	}

	private sourceLabel(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.source === "heartbeat" ? "Created by you" : "Created by agent";
	}

	private defaultScheduledName(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.source === "heartbeat" ? "Your scheduled task" : "Agent-created scheduled task";
	}

	private singleLine(value: string): string {
		return value.replace(/\s+/g, " ").trim();
	}

	private formatTimestamp(value: string): string {
		const parsed = new Date(value);
		if (!Number.isFinite(parsed.getTime())) return value;
		return parsed.toLocaleString(undefined, {
			year: "numeric", month: "2-digit", day: "2-digit",
			hour: "2-digit", minute: "2-digit", timeZoneName: "short",
		});
	}
}
