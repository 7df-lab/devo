import { stripVTControlCharacters } from "node:util";
import { type Component, type Focusable, getKeybindings, truncateToWidth } from "@earendil-works/pi-tui";
import type { AgentConnectionShellTask } from "../../agent-connection/types.js";
import { shouldTreatAsBack } from "./modal-back.js";
import { theme } from "../theme/theme.js";
import type { ActivityLoadState } from "./subagent-summary-line.js";

export interface ShellActivityPanelOptions {
	getTasks: () => readonly AgentConnectionShellTask[];
	getRows: () => number;
	getError: () => string | undefined;
	getState?: () => ActivityLoadState;
	onClose: () => void;
	requestRender: () => void;
}

/** Read-only process activity. The owner refreshes from the session's task registry. */
export class ShellActivityPanel implements Component, Focusable {
	focused = false;
	private selectedId: string | undefined;
	private details = false;
	private outputOffset = 0;

	constructor(private readonly options: ShellActivityPanelOptions) {}

	invalidate(): void {}

	handleInput(data: string): void {
		const keys = getKeybindings();
		if (keys.matches(data, "tui.select.cancel") || shouldTreatAsBack(data)) {
			if (this.details) {
				this.details = false;
				this.outputOffset = 0;
				this.options.requestRender();
			} else this.options.onClose();
			return;
		}
		const tasks = this.options.getTasks();
		const found = tasks.findIndex((task) => task.id === this.selectedId);
		const index = Math.max(0, found);
		const direction = keys.matches(data, "tui.select.up") ? -1 : keys.matches(data, "tui.select.down") ? 1 : 0;
		if (direction !== 0) {
			if (this.details) {
				const lineCount = this.outputLines(tasks[index]).length;
				this.outputOffset = Math.max(0, Math.min(lineCount - 1, this.outputOffset + direction));
			} else {
				this.selectedId = tasks[Math.max(0, Math.min(tasks.length - 1, index + direction))]?.id;
			}
			this.options.requestRender();
			return;
		}
		if (keys.matches(data, "tui.select.confirm") && tasks[index]) {
			this.selectedId = tasks[index].id;
			this.details = true;
			this.outputOffset = 0;
			this.options.requestRender();
		}
	}

	render(width: number): string[] {
		if (width < 1) return [];
		const tasks = this.options.getTasks();
		if (!tasks.some((task) => task.id === this.selectedId)) {
			this.selectedId = tasks[0]?.id;
			this.details = false;
			this.outputOffset = 0;
		}
		const rows = Math.max(1, this.options.getRows());
		const lines = [theme.fg("accent", "Shells")];
		const state = this.options.getState?.() ?? "ready";
		const error = this.options.getError();
		if (error || state === "error") {
			lines.push(theme.fg("error", `Unable to refresh shell activity${error ? `: ${this.singleLine(error)}` : ""}`));
		} else if (state === "loading") {
			lines.push(theme.fg("muted", "Loading shell activity…"));
		} else if (state === "unavailable") {
			lines.push(theme.fg("muted", "Shell activity is unavailable on this connection."));
		}
		const task = tasks.find((entry) => entry.id === this.selectedId);
		if (state === "ready" && !error && tasks.length === 0) {
			lines.push(theme.fg("muted", "No background shell tasks in this session."));
		} else if (task && this.details) {
			lines.push(this.singleLine(task.command), theme.fg("muted", `${task.state}${task.exitCode === undefined ? "" : ` · exit ${task.exitCode}`}`));
			if (task.cwd) lines.push(theme.fg("dim", this.singleLine(task.cwd)));
			const output = this.outputLines(task);
			const available = Math.max(0, rows - lines.length - 1);
			this.outputOffset = Math.min(this.outputOffset, Math.max(0, output.length - 1));
			lines.push(...output.slice(this.outputOffset, this.outputOffset + available));
		} else if (tasks.length > 0) {
			const selected = tasks.findIndex((entry) => entry.id === this.selectedId);
			const available = Math.max(0, rows - lines.length - 1);
			const start = Math.max(0, Math.min(selected - Math.floor(available / 2), tasks.length - available));
			for (const task of tasks.slice(start, start + available)) {
				const text = `${task.id === this.selectedId ? "›" : " "} ${task.state} · ${this.singleLine(task.command)}`;
				lines.push(theme.fg(task.id === this.selectedId ? "accent" : "muted", text));
			}
		}
		lines.push(theme.fg("dim", this.details ? "↑/↓ scroll · ←/esc back" : "←/esc close · ↑/↓ select · enter details"));
		const bounded = lines.length > rows ? [...lines.slice(0, Math.max(0, rows - 1)), lines[lines.length - 1]!] : lines;
		return bounded.map((line) => truncateToWidth(line, width, "…"));
	}

	private outputLines(task: AgentConnectionShellTask | undefined): string[] {
		if (!task?.output) return ["No captured output."];
		return stripVTControlCharacters(task.output).replace(/\r\n/g, "\n").split("\n").map((line) => this.singleLine(line));
	}

	private singleLine(value: string): string {
		return stripVTControlCharacters(value).replace(/[\x00-\x1f\x7f-\x9f]/g, " ");
	}
}
