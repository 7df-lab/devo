import { type Component, type Focusable, getKeybindings, Spacer, TruncatedText } from "@earendil-works/pi-tui";
import type { AgentScheduledManagementAction } from "../../../core/cron-jobs.js";
import type { AgentConnectionScheduledTask } from "../../agent-connection/types.js";
import { theme } from "../theme/theme.js";
import { keyHint } from "./keybinding-hints.js";
import { getMenuListLayout, MenuList, MenuPanel, MenuRow } from "./menu-panel.js";
import { shouldTreatAsBack } from "./modal-back.js";

const SCHEDULED_PANEL_MAX_WIDTH = 72;
const PREFERRED_VISIBLE_SCHEDULEDS = 8;
const SCHEDULED_LIST_RESERVED_ROWS = 7;
const SCHEDULED_SCROLL_INDICATOR_ROWS = 1;

type ScheduledManagerMode = { type: "list" } | { type: "actions"; scheduledId: string; selectedIndex: number };

export interface ScheduledManagerOptions {
	getScheduledTasks: () => readonly AgentConnectionScheduledTask[];
	getRows: () => number;
	onAction: (scheduled: AgentConnectionScheduledTask, action: AgentScheduledManagementAction) => Promise<void>;
	onClose: () => void;
	requestRender: () => void;
}

export class ScheduledManagerComponent implements Component, Focusable {
	private selectedScheduledId: string | undefined;
	private mode: ScheduledManagerMode = { type: "list" };
	private busy = false;
	private error: string | undefined;
	private _focused = false;

	constructor(private readonly options: ScheduledManagerOptions) {}

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
		if (this.busy) return;
		const keybindings = getKeybindings();
		if (keybindings.matches(data, "tui.select.cancel") || keybindings.matches(data, "app.heartbeats.open")) {
			this.options.onClose();
			return;
		}
		if (shouldTreatAsBack(data)) {
			if (this.mode.type === "list") {
				this.options.onClose();
			} else {
				this.mode = { type: "list" };
				this.error = undefined;
				this.options.requestRender();
			}
			return;
		}
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
		const panel = this.mode.type === "list" ? this.createScheduledListPanel() : this.createActionPanel(this.mode);
		const safeWidth = Math.max(1, width);
		const panelWidth = Math.min(safeWidth, SCHEDULED_PANEL_MAX_WIDTH);
		const leftPadding = Math.max(0, Math.floor((safeWidth - panelWidth) / 2));
		const rightPadding = Math.max(0, safeWidth - panelWidth - leftPadding);
		return panel.render(panelWidth).map((line) => " ".repeat(leftPadding) + line + " ".repeat(rightPadding));
	}

	private createScheduledListPanel(): MenuPanel {
		const active = this.scheduledTasks.filter((scheduled) => scheduled.job.status === "active").length;
		const paused = this.scheduledTasks.length - active;
		const countLabel = `${this.scheduledTasks.length} scheduled task${this.scheduledTasks.length === 1 ? "" : "s"}${paused ? ` · ${paused} paused` : ""}`;
		const panel = new MenuPanel({
			title: "Scheduled",
			subtitle: `${countLabel}. Select a scheduled task to manage.`,
		});
		const list = new MenuList({ compact: this.getListLayout().compact });
		this.populateScheduledList(list);
		panel.addChild(list);
		if (this.error) {
			panel.addChild(new Spacer(1));
			panel.addChild(new TruncatedText(theme.fg("error", `Error: ${this.error}`)));
		}
		panel.addChild(new Spacer(1));
		panel.addChild(new TruncatedText(this.closeHint()));
		return panel;
	}

	private populateScheduledList(list: MenuList): void {
		const scheduledTasks = this.scheduledTasks;
		if (scheduledTasks.length === 0) {
			list.addChild(new TruncatedText(theme.fg("muted", "No running or paused scheduled tasks"), 1, 0));
			return;
		}
		const selectedIndex = this.getSelectedIndex(scheduledTasks);
		const visibleItems = this.getListLayout().visibleItems;
		const startIndex = Math.max(
			0,
			Math.min(selectedIndex - Math.floor(visibleItems / 2), scheduledTasks.length - visibleItems),
		);
		const endIndex = Math.min(startIndex + visibleItems, scheduledTasks.length);

		for (let index = startIndex; index < endIndex; index++) {
			const scheduled = scheduledTasks[index];
			if (!scheduled) continue;
			const source = this.sourceLabel(scheduled);
			const label = scheduled.job.label?.trim();
			const delivery = scheduled.job.deliveryMode === "follow_up" ? "follow-up" : "steer";
			const details = scheduled.job.lastError
				? `${source} · error: ${this.singleLine(scheduled.job.lastError)}`
				: `${source} · ${this.sessionLabel(scheduled)} · ${scheduled.job.schedule.expression} · ${delivery}`;
			list.addChild(
				new MenuRow({
					primary: label || this.singleLine(scheduled.job.prompt) || this.defaultScheduledName(scheduled),
					secondary: details,
					meta: this.formatStatus(scheduled),
					selected: index === selectedIndex,
				}),
			);
		}

		if (startIndex > 0 || endIndex < scheduledTasks.length) {
			list.addChild(new TruncatedText(theme.fg("muted", `  (${selectedIndex + 1}/${scheduledTasks.length})`), 1, 0));
		}
	}

	private createActionPanel(mode: Exclude<ScheduledManagerMode, { type: "list" }>): MenuPanel {
		const scheduled = this.findScheduled(mode.scheduledId);
		if (!scheduled) {
			return new MenuPanel({ title: "Scheduled", subtitle: "This scheduled task is no longer available." });
		}
		const name = scheduled.job.label?.trim() || this.defaultScheduledName(scheduled);
		const panel = new MenuPanel({
			title: name,
			subtitle: this.singleLine(scheduled.job.prompt),
		});
		panel.addChild(new TruncatedText(theme.fg("muted", this.formatScheduledDetails(scheduled))));
		panel.addChild(new Spacer(1));
		if (this.error) {
			panel.addChild(new TruncatedText(theme.fg("error", `Error: ${this.error}`)));
			panel.addChild(new Spacer(1));
		}
		const list = new MenuList();
		for (const [index, action] of this.availableActions(scheduled).entries()) {
			list.addChild(
				new MenuRow({
					primary: action.label,
					secondary: this.actionDescription(action.action),
					selected: index === mode.selectedIndex,
				}),
			);
		}
		panel.addChild(list);
		panel.addChild(new Spacer(1));
		panel.addChild(new TruncatedText(this.detailHint()));
		return panel;
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
		this.busy = true;
		this.error = undefined;
		this.options.requestRender();
		try {
			await this.options.onAction(scheduled, action);
			this.mode = { type: "list" };
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
		} finally {
			this.busy = false;
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

	private getListLayout() {
		return getMenuListLayout({
			getRows: this.options.getRows,
			preferredVisibleItems: PREFERRED_VISIBLE_SCHEDULEDS,
			totalItems: this.scheduledTasks.length,
			reservedRows: SCHEDULED_LIST_RESERVED_ROWS + (this.error ? 2 : 0),
			comfortableItemRows: 3,
			compactItemRows: 2,
			scrollIndicatorRows: SCHEDULED_SCROLL_INDICATOR_ROWS,
		});
	}

	private formatStatus(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.status === "active" ? theme.fg("success", "active") : theme.fg("warning", "paused");
	}

	private formatScheduledDetails(scheduled: AgentConnectionScheduledTask): string {
		const delivery = scheduled.job.deliveryMode === "follow_up" ? "follow-up" : "steer";
		const next = scheduled.job.nextRunAt ? this.formatTimestamp(scheduled.job.nextRunAt) : "—";
		return `${this.sourceLabel(scheduled)} · ${this.sessionLabel(scheduled)} · ${scheduled.job.status} · ${scheduled.job.schedule.expression} · ${delivery} · next ${next} · ${scheduled.job.runCount} runs`;
	}

	private sourceLabel(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.source === "heartbeat" ? "Created by you" : "Created by agent";
	}

	private defaultScheduledName(scheduled: AgentConnectionScheduledTask): string {
		return scheduled.job.source === "heartbeat" ? "Your scheduled task" : "Agent-created scheduled task";
	}

	private closeHint(): string {
		return keyHint("tui.select.cancel", "close", { primaryOnly: true });
	}

	private detailHint(): string {
		return `${keyHint("app.modal.back", "back")}  ${keyHint("tui.select.cancel", "close", { primaryOnly: true })}`;
	}

	private actionDescription(action: AgentScheduledManagementAction): string {
		switch (action) {
			case "pause":
				return "Stop deliveries until resumed";
			case "resume":
				return "Continue scheduled deliveries";
			case "stop":
				return "Permanently remove this scheduled task";
		}
	}

	private singleLine(value: string): string {
		return value.replace(/\s+/g, " ").trim();
	}

	private formatTimestamp(value: string): string {
		const parsed = new Date(value);
		if (!Number.isFinite(parsed.getTime())) return value;
		return parsed.toISOString().slice(0, 16).replace("T", " ");
	}
}
