import { Container, type Focusable, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import stripAnsi from "strip-ansi";
import type { AgentConnectionWebSearchChoice, AgentConnectionWebStatus, WebSearchChoice } from "../../agent-connection/types.js";
import { theme } from "../theme/theme.js";
import { keyHint } from "./keybinding-hints.js";

function sameChoice(left: WebSearchChoice | undefined, right: WebSearchChoice): boolean {
	if (!left || left.kind === "inherit" || right.kind === "inherit") return left?.kind === right.kind;
	const leftBackend = left.kind === "unified" ? left.backend : left;
	const rightBackend = right.kind === "unified" ? right.backend : right;
	return leftBackend.kind === rightBackend.kind
		&& (leftBackend.kind !== "mcp" || (rightBackend.kind === "mcp" && leftBackend.serverId === rightBackend.serverId && leftBackend.toolName === rightBackend.toolName));
}

function singleLine(text: string): string {
	return stripAnsi(text).replace(/[\x00-\x1f\x7f-\x9f]/g, " ");
}

type WebMenu = "root" | "builtin" | "mcp";
interface WebMenuRow {
	label: string;
	choice?: WebSearchChoice;
	note?: string;
	keyStatus?: AgentConnectionWebSearchChoice["keyStatus"];
	unavailable?: boolean;
	submenu?: WebMenu;
	active?: boolean;
}

/** Borderless inline /web picker. Nested menus keep the slash command out of chat. */
export class WebSearchSelectorComponent extends Container implements Focusable {
	private menu: WebMenu = "root";
	private selectedIndex = 0;
	private isFocused = false;
	private choices: AgentConnectionWebSearchChoice[];
	private status: AgentConnectionWebStatus;

	constructor(
		private currentChoice: WebSearchChoice | undefined,
		choices: AgentConnectionWebSearchChoice[],
		status: AgentConnectionWebStatus,
		private readonly onSelect: (choice: WebSearchChoice, label: string) => void,
		private readonly onCancel: () => void,
		private readonly onUnavailable: (message: string) => void = () => {},
	) {
		super();
		this.choices = choices;
		this.status = status;
		this.selectedIndex = Math.max(0, this.getRows().findIndex((row) => row.active));
	}

	get focused(): boolean {
		return this.isFocused;
	}

	set focused(value: boolean) {
		this.isFocused = value;
	}

	updateStatus(status: AgentConnectionWebStatus): void {
		const selected = this.getRows()[this.selectedIndex];
		this.status = status;
		this.choices = status.searchChoices ?? this.choices;
		this.currentChoice = status.searchChoice;
		const rows = this.getRows();
		const matched = rows.findIndex((row) => row.submenu
			? row.submenu === selected?.submenu
			: row.choice !== undefined && sameChoice(selected?.choice, row.choice));
		this.selectedIndex = matched >= 0 ? matched : Math.min(this.selectedIndex, Math.max(0, rows.length - 1));
	}

	hasPendingProbe(): boolean {
		return this.choices.some(({ choice, note }) => this.backendOf(choice)?.kind === "provider" && note?.toLowerCase().includes("checking"));
	}

	handleInput(data: string): void {
		if (data === "\x1b" || data === "\x1b[5~") {
			if (this.menu === "root") this.onCancel();
			else {
				const submenu = this.menu;
				this.menu = "root";
				this.selectedIndex = Math.max(0, this.getRows().findIndex((row) => row.submenu === submenu));
			}
			return;
		}
		const rows = this.getRows();
		if (rows.length === 0) return;
		if (data === "\x1b[A" || data === "k") {
			this.selectedIndex = (this.selectedIndex - 1 + rows.length) % rows.length;
		} else if (data === "\x1b[B" || data === "j") {
			this.selectedIndex = (this.selectedIndex + 1) % rows.length;
		} else if (data === "\r" || data === "\n") {
			this.activate(rows[this.selectedIndex]);
		}
	}

	render(width: number): string[] {
		const rows = this.getRows();
		const lines: string[] = [];
		if (this.menu !== "root") {
			lines.push(theme.fg("dim", this.menu === "builtin" ? "Built-in" : "My MCP tools"), "");
		} else if (this.status.currentBackend) {
			lines.push(theme.fg("dim", `Current: ${singleLine(this.status.currentBackend)}`), "");
		}
		const columnWidth = Math.min(width, 82);
		for (const [index, row] of rows.entries()) {
			const isSelected = index === this.selectedIndex;
			const marker = isSelected ? theme.fg("accent", "›") : " ";
			const activeMark = row.active ? theme.fg("success", "✓") : " ";
			const keyNote = row.keyStatus ? this.renderKeyStatus(row.keyStatus) : "";
			const note = row.note ? theme.fg("dim", singleLine(row.note)) : "";
			const trailing = [keyNote, row.submenu ? theme.fg("dim", "›") : "", note].filter(Boolean).join("  ");
			const label = theme.fg(isSelected ? "accent" : row.unavailable ? "dim" : "text", singleLine(row.label));
			const left = truncateToWidth(`${marker} ${activeMark} ${label}`, Math.max(1, columnWidth - (trailing ? visibleWidth(trailing) + 2 : 0)), "…");
			const text = trailing ? `${left}${" ".repeat(Math.max(2, columnWidth - visibleWidth(left) - visibleWidth(trailing)))}${trailing}` : left;
			lines.push(truncateToWidth(text, Math.max(1, width), "…"));
		}
		lines.push("");
		const keyHintRow = this.keySetupHint(rows[this.selectedIndex]);
		if (keyHintRow) lines.push(theme.fg("warning", keyHintRow));
		const action = this.menu === "root" ? "close" : "back";
		lines.push(theme.fg("dim", `↑↓ move  ${keyHint("tui.select.confirm", "select", { primaryOnly: true })}  ${keyHint("tui.select.cancel", action, { primaryOnly: true })}`));
		return lines.map((line) => truncateToWidth(line, Math.max(1, width), "…"));
	}

	private getRows(): WebMenuRow[] {
		const provider = this.choices.find((entry) => this.backendOf(entry.choice)?.kind === "provider");
		const builtins = this.choices.filter((entry) => this.groupOf(entry) === "builtin");
		const mcp = this.choices.filter((entry) => this.groupOf(entry) === "custom");
		if (this.menu === "builtin") return builtins.map((entry) => ({ ...this.entryRow(entry), label: this.supplierName(entry) }));
		if (this.menu === "mcp") return mcp.map((entry) => this.entryRow(entry));

		const rows: WebMenuRow[] = [];
		const activeBuiltin = builtins.find((entry) => sameChoice(this.activeChoice(), entry.choice));
		const activeMcp = mcp.find((entry) => sameChoice(this.activeChoice(), entry.choice));
		if (provider) rows.push(this.entryRow(provider));
		if (builtins.length) rows.push({ label: activeBuiltin ? `Built-in · ${this.supplierName(activeBuiltin)}` : "Built-in", submenu: "builtin", active: !!activeBuiltin });
		if (mcp.length) rows.push({ label: activeMcp ? `My MCP tools · ${activeMcp.label}` : "My MCP tools", submenu: "mcp", active: !!activeMcp });
		const off = this.choices.find((entry) => this.backendOf(entry.choice)?.kind === "off");
		if (off) rows.push(this.entryRow(off));
		const inherit = this.choices.find((entry) => entry.choice.kind === "inherit");
		if (inherit && this.currentChoice && this.currentChoice.kind !== "inherit") {
			rows.push({
				...this.entryRow(inherit),
				label: "Use configured backend",
				note: this.status.configuredBackend ? `→ ${this.status.configuredBackend}` : undefined,
			});
		}
		return rows;
	}

	private entryRow(entry: AgentConnectionWebSearchChoice): WebMenuRow {
		return {
			label: entry.label,
			choice: entry.choice,
			note: entry.note,
			keyStatus: entry.keyStatus,
			unavailable: entry.unavailable,
			active: sameChoice(this.activeChoice(), entry.choice),
		};
	}

	private backendOf(choice: WebSearchChoice): Exclude<WebSearchChoice, { kind: "inherit" } | { kind: "unified" }> | undefined {
		return choice.kind === "unified" ? choice.backend : choice.kind === "inherit" ? undefined : choice;
	}

	private groupOf(entry: AgentConnectionWebSearchChoice): "builtin" | "custom" | undefined {
		if (entry.group) return entry.group;
		const choice = entry.choice;
		if (choice.kind === "unified") {
			if (choice.backend.kind !== "mcp") return undefined;
			return choice.backend.serverId.startsWith("devo:builtin:") ? "builtin" : "custom";
		}
		if (choice.kind === "mcp") return choice.serverId.startsWith("devo:builtin:") ? "builtin" : "custom";
		return undefined;
	}

	private activeChoice(): WebSearchChoice | undefined {
		const active = this.status.activeChoice ?? (this.status.configuredBackend === undefined ? this.currentChoice : undefined);
		return active?.kind === "inherit" ? undefined : active;
	}

	private supplierName(entry: AgentConnectionWebSearchChoice): string {
		return entry.label.replace(/ · Built-in(?: MCP)?$/, "");
	}

	private keySetupHint(row: WebMenuRow | undefined): string | undefined {
		if (row?.keyStatus === "required") return `Save a ${singleLine(row.label)} key in /login to use it.`;
		if (row?.keyStatus === "rejected") return `Replace the ${singleLine(row.label)} key in /login.`;
		return undefined;
	}

	private renderKeyStatus(status: NonNullable<AgentConnectionWebSearchChoice["keyStatus"]>): string {
		switch (status) {
			case "saved": return theme.fg("success", "Key saved");
			case "required": return theme.fg("warning", "Key required");
			case "rejected": return theme.fg("error", "Key rejected");
		}
	}

	private activate(row: WebMenuRow | undefined): void {
		if (!row) return;
		if (row.submenu) {
			this.menu = row.submenu;
			this.selectedIndex = Math.max(0, this.getRows().findIndex((entry) => entry.active));
			return;
		}
		if (this.keySetupHint(row)) return;
		if (!row.choice || row.unavailable) {
			if (row.unavailable) this.onUnavailable(row.note ?? `${row.label} is unavailable.`);
			return;
		}
		this.onSelect(row.choice, singleLine(row.choice.kind === "inherit" ? this.status.configuredBackend ?? "configured backend" : row.label));
	}
}
