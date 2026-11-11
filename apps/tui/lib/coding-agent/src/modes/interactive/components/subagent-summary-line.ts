import { type Component, type Focusable, getKeybindings, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { AgentConnectionRlmChildAgentSnapshot } from "../../agent-connection/index.js";
import { collectSubagentDescendantSummaries } from "../../agents-view/agents-view-state.js";
import { type AgentRosterStatus, classifyAgentStatus } from "../../daemon/agent-roster.js";
import { classifySessionRosterStatus, type SessionSummary } from "../../daemon/daemon-session-list.js";
import { theme } from "../theme/theme.js";
import { keyText } from "./keybinding-hints.js";

export interface SubagentSummaryCounts {
	total: number;
	running: number;
	idle: number;
	inactive: number;
}

export function classifySubagentSnapshotStatus(child: AgentConnectionRlmChildAgentSnapshot): AgentRosterStatus {
	// Activity implies a live session; the in-process connection never stamps activeSessionId.
	const resident = child.activeSessionId !== undefined || child.activity !== undefined;
	const busy = child.status === "running" || child.status === "queued" || child.activity !== undefined;
	return classifyAgentStatus({
		resident,
		queuedChild: !resident && busy,
		busy,
	});
}

/** Status counts over every snapshot descending from `parentId`; the recursive roster carries the whole subtree. */
export function countSubtreeSubagentStatuses(
	children: Iterable<AgentConnectionRlmChildAgentSnapshot>,
	parentId: string | undefined,
): SubagentSummaryCounts {
	const counts: SubagentSummaryCounts = { total: 0, running: 0, idle: 0, inactive: 0 };
	const knownNodes = new Set<string | undefined>([parentId]);
	const pending = [...children].filter((child) => child.status !== "cancelled");
	let matched = true;
	while (matched) {
		matched = false;
		for (let index = pending.length - 1; index >= 0; index--) {
			const child = pending[index]!;
			if (!knownNodes.has(child.parentId)) continue;
			pending.splice(index, 1);
			matched = true;
			counts.total += 1;
			counts[classifySubagentSnapshotStatus(child)] += 1;
			knownNodes.add(child.id);
		}
	}
	return counts;
}

export function countRosterSubagentStatuses(
	summaries: Iterable<SessionSummary>,
	parent: { activeSessionId?: string | undefined; sessionId?: string | undefined; sessionFile?: string | undefined },
): SubagentSummaryCounts {
	const counts: SubagentSummaryCounts = { total: 0, running: 0, idle: 0, inactive: 0 };
	// The daemon roster spans every tree: count live rows in this session's subtree, at any depth.
	for (const child of collectSubagentDescendantSummaries(summaries, parent)) {
		if (child.lifecycle !== "live") continue;
		counts.total += 1;
		counts[child.rosterStatus ?? classifySessionRosterStatus(child)] += 1;
	}
	return counts;
}

export type ActivityLoadState = "ready" | "loading" | "unavailable" | "error";

export interface HeartbeatSummaryCounts {
	total: number;
	active: number;
	paused: number;
	/** Epoch milliseconds of the next active heartbeat, if known. */
	nextRunAt?: number;
}

export interface ShellSummaryCounts {
	total: number;
	running: number;
	/** Omit when the backend cannot distinguish persistent shells. */
	persistent?: number;
}

/** Session-scoped activity entry points; empty groups remain visible and navigable. */
export class SubagentSummaryLine implements Component, Focusable {
	focused = false;
	private counts: SubagentSummaryCounts = { total: 0, running: 0, idle: 0, inactive: 0 };
	private heartbeats: HeartbeatSummaryCounts = { total: 0, active: 0, paused: 0 };
	private shells: ShellSummaryCounts = { total: 0, running: 0 };
	private heartbeatState: ActivityLoadState = "loading";
	private shellState: ActivityLoadState = "loading";
	private openable = false;
	private selected = 0;

	onOpen?: () => void;
	onOpenHeartbeats?: () => void;
	onOpenShells?: () => void;
	onCancel?: () => void;
	onChatAction?: (data: string) => void;

	constructor(
		private readonly getLocationLabel: () => string | undefined = () => undefined,
		private readonly getContextLabel: () => string | undefined = () => undefined,
		private readonly getOverrideLabel: () => string | undefined = () => undefined,
		private readonly getPickerOpen: () => boolean = () => false,
	) {}

	setSubagentCounts(counts: SubagentSummaryCounts): void {
		this.counts = { ...counts };
	}

	setHeartbeatCounts(counts: HeartbeatSummaryCounts): void {
		this.heartbeats = { ...counts };
		this.heartbeatState = "ready";
	}

	setShellCounts(counts: ShellSummaryCounts): void {
		this.shells = { ...counts };
		this.shellState = "ready";
	}

	setHeartbeatState(state: ActivityLoadState): void {
		this.heartbeatState = state;
	}

	setShellState(state: ActivityLoadState): void {
		this.shellState = state;
	}

	setOpenable(openable: boolean): void {
		this.openable = openable;
	}

	isSelectable(): boolean {
		return !this.getPickerOpen();
	}

	handleInput(data: string): void {
		if (!this.isSelectable()) return;
		const keybindings = getKeybindings();
		if (keybindings.matches(data, "app.agents.open")) {
			this.onOpen?.();
			return;
		}
		if (keybindings.matches(data, "tui.select.confirm")) {
			[this.onOpen, this.onOpenHeartbeats, this.onOpenShells][this.selected]?.();
			return;
		}
		if (keybindings.matches(data, "tui.select.cancel") || keybindings.matches(data, "app.agents.back")) {
			this.onCancel?.();
			return;
		}
		if (keybindings.matches(data, "tui.select.up") || keybindings.matches(data, "tui.editor.cursorLeft")) {
			if (this.selected === 0) this.onCancel?.();
			else this.selected -= 1;
			return;
		}
		if (
			keybindings.matches(data, "tui.select.down") ||
			keybindings.matches(data, "tui.editor.cursorRight") ||
			keybindings.matches(data, "tui.input.tab")
		) {
			this.selected = (this.selected + 1) % 3;
			return;
		}
		this.onChatAction?.(data);
	}

	render(width: number): string[] {
		if (this.getPickerOpen() || width < 1) return [];
		const lines = this.renderInfoLine(width);
		const heartbeatCount = this.heartbeatState === "ready" ? String(this.heartbeats.total) : this.heartbeatState;
		const shellCount = this.shellState === "ready" ? String(this.shells.running) : this.shellState;
		const groups = [
			`Subagents ${this.counts.running}`,
			`Heartbeats ${heartbeatCount}`,
			`Shells ${shellCount}`,
		];
		if (!this.focused) {
			const compact = groups.join(" · ");
			// Narrow terminals retain each group instead of silently clipping the last group.
			const rows = visibleWidth(compact) <= width ? [compact] : groups;
			return [...lines, ...rows.map((row) => theme.fg("muted", truncateToWidth(row, width, "…")))];
		}
		let nextHeartbeat = "";
		if (this.heartbeats.nextRunAt !== undefined && Number.isFinite(this.heartbeats.nextRunAt)) {
			const seconds = Math.max(0, Math.ceil((this.heartbeats.nextRunAt - Date.now()) / 1000));
			nextHeartbeat = ` · next ${seconds}s`;
		}
		const details = [
			`Subagents · ${this.counts.running} running · ${this.counts.idle} idle · ${this.counts.inactive} inactive${this.openable ? "" : " · details unavailable"}`,
			this.heartbeatState === "ready"
				? `Heartbeats · ${this.heartbeats.active} active · ${this.heartbeats.paused} paused${nextHeartbeat}`
				: `Heartbeats · ${this.heartbeatState}`,
			this.shellState === "ready"
				? `Shells · ${this.shells.running} running · ${this.shells.total} total${this.shells.persistent === undefined ? "" : ` · ${this.shells.persistent} persistent`}`
				: `Shells · ${this.shellState}`,
		];
		for (const [index, detail] of details.entries()) {
			const row = truncateToWidth(`${index === this.selected ? "›" : " "} ${detail}`, width, "…");
			lines.push(index === this.selected ? theme.bg("selectedBg", row) : theme.fg("muted", row));
		}
		lines.push(theme.fg("dim", truncateToWidth(`↑/↓/tab select · ${keyText("tui.select.confirm")} open · ${keyText("tui.select.cancel")} back`, width, "…")));
		return lines;
	}

	private renderInfoLine(width: number): string[] {
		const overrideLabel = this.getOverrideLabel()?.trim();
		const locationLabel = this.getLocationLabel()?.trim();
		const contextLabel = this.getContextLabel()?.trim();
		const left = overrideLabel || locationLabel || "";
		if (!left && !contextLabel) return [];
		const safeWidth = Math.max(1, width);
		const right = contextLabel ?? "";
		const gap = left && right ? 2 : 0;
		const rightWidth = Math.min(visibleWidth(right), Math.max(0, safeWidth - gap));
		const leftWidth = Math.max(0, safeWidth - rightWidth - gap);
		const renderedLeft = truncateToWidth(left, leftWidth, "…");
		const renderedRight = truncateToWidth(right, rightWidth, "…");
		const padding = Math.max(0, safeWidth - visibleWidth(renderedLeft) - visibleWidth(renderedRight));
		return [theme.fg("muted", `${renderedLeft}${" ".repeat(padding)}${renderedRight}`)];
	}

	invalidate(): void {
		// Render output is derived from current state; the owner schedules redraws.
	}
}
