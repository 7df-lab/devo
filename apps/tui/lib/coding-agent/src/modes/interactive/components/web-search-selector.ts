import { Container, CURSOR_MARKER, type Focusable, Input, matchesKey, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import stripAnsi from "strip-ansi";
import { BUILTIN_WEB_CREDENTIAL_IDS, type BuiltinWebCredentialId } from "../../../core/websearch-credential.js";
import type { AgentConnectionWebSearchChoice, AgentConnectionWebStatus, WebSearchChoice } from "../../agent-connection/types.js";
import { theme } from "../theme/theme.js";
import { keyHint, rawKeyHint } from "./keybinding-hints.js";

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

interface WebKeyEntry {
	input: Input;
	choice: WebSearchChoice;
	credentialId: BuiltinWebCredentialId;
	label: string;
	saving: boolean;
	error?: string;
}

interface WebKeyEntryActions {
	// undefined means the panel/session was closed before the save could select a backend.
	save: (credentialId: BuiltinWebCredentialId, apiKey: string) => Promise<AgentConnectionWebStatus | undefined>;
	requestRender: () => void;
}

/** Borderless inline /web picker. Nested menus keep the slash command out of chat. */
export class WebSearchSelectorComponent extends Container implements Focusable {
	private menu: WebMenu = "root";
	private selectedIndex = 0;
	private isFocused = false;
	private choices: AgentConnectionWebSearchChoice[];
	private status: AgentConnectionWebStatus;
	private keyEntry?: WebKeyEntry;

	constructor(
		private currentChoice: WebSearchChoice | undefined,
		choices: AgentConnectionWebSearchChoice[],
		status: AgentConnectionWebStatus,
		private readonly onSelect: (choice: WebSearchChoice, label: string) => void,
		private readonly onCancel: () => void,
		private readonly onUnavailable: (message: string) => void = () => {},
		private readonly keyEntryActions?: WebKeyEntryActions,
	) {
		super();
		this.choices = choices;
		this.status = status;
		this.selectedIndex = Math.max(0, this.getRows().findIndex((row) => row.active && row.choice?.kind !== "inherit"));
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
		if (matchesKey(data, "escape")) {
			this.keyEntry = undefined;
			this.onCancel();
			return;
		}
		if (this.keyEntry) {
			if (!this.keyEntry.saving) {
				if (matchesKey(data, "left") && !this.keyEntry.input.getValue()) this.keyEntry = undefined;
				else this.keyEntry.input.handleInput(data);
			}
			return;
		}
		if (matchesKey(data, "left")) {
			if (this.menu !== "root") {
				const submenu = this.menu;
				this.menu = "root";
				this.selectedIndex = Math.max(0, this.getRows().findIndex((row) => row.submenu === submenu));
			}
			return;
		}
		const rows = this.getRows();
		if (rows.length === 0) return;
		if (matchesKey(data, "up") || data === "k") {
			this.selectedIndex = (this.selectedIndex - 1 + rows.length) % rows.length;
		} else if (matchesKey(data, "down") || data === "j") {
			this.selectedIndex = (this.selectedIndex + 1) % rows.length;
		} else if (matchesKey(data, "enter") || data === "\n") {
			this.activate(rows[this.selectedIndex]);
		}
	}

	render(width: number): string[] {
		if (this.keyEntry) return this.renderKeyEntry(this.keyEntry, width);
		const rows = this.getRows();
		const lines: string[] = [];
		if (this.menu !== "root") {
			lines.push(theme.fg("dim", this.menu === "builtin" ? "Built-in" : "My MCP tools"), "");
		} else if (this.status.currentBackend && !this.activeChoice() && this.currentChoice && this.currentChoice.kind !== "inherit") {
			// A legacy search-only override does not match a unified picker entry.
			lines.push(`${theme.fg("success", "✓")} ${theme.fg("dim", "Existing setup")}`, "");
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
		const selected = rows[this.selectedIndex];
		const keyAction = selected?.keyStatus === "rejected" ? "replace key" : "add key";
		const confirm = keyHint("tui.select.confirm", this.credentialToConfigure(selected) ? keyAction : "select", { primaryOnly: true });
		const back = this.menu === "root" ? "" : `  ${rawKeyHint("left", "back")}`;
		lines.push(theme.fg("dim", `↑↓ move  ${confirm}${back}  ${rawKeyHint("esc", "close")}`));
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
		const usingDefault = !this.currentChoice || this.currentChoice.kind === "inherit";
		if (inherit && (!usingDefault || (this.status.currentBackend && !this.activeChoice()))) {
			// Keep named-backend previews, but don't surface split routing here.
			const configured = this.choices.find((entry) =>
				entry.choice.kind !== "inherit" && entry.label === this.status.configuredBackend);
			rows.push({
				choice: inherit.choice,
				label: usingDefault ? "Default setup" : "Use default setup",
				active: usingDefault,
				note: !usingDefault && configured ? `→ ${this.supplierName(configured)}` : undefined,
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

	private credentialToConfigure(row: WebMenuRow | undefined): BuiltinWebCredentialId | undefined {
		if (!row?.choice || (row.keyStatus !== "required" && row.keyStatus !== "rejected")) return undefined;
		const backend = this.backendOf(row.choice);
		if (backend?.kind !== "mcp") return undefined;
		return BUILTIN_WEB_CREDENTIAL_IDS.find((id) => backend.serverId === `devo:builtin:${id}`);
	}

	private renderKeyEntry(entry: WebKeyEntry, width: number): string[] {
		// Never render the Input component itself: it displays plaintext. Use
		// its editing and paste handling, and render only bullets and a cursor.
		const available = Math.max(1, Math.min(width, 82) - 3);
		const position = entry.input.getCursor();
		const before = Math.min(position, available - 1);
		const after = Math.min(entry.input.getValue().length - position, available - before - 1);
		const cursor = this.focused && !entry.saving ? CURSOR_MARKER : "";
		const field = `${theme.fg("accent", "›")} ${"•".repeat(before)}${cursor}${"•".repeat(after)}`;
		const lines = [theme.fg("text", `${entry.label} API key`), "", field, ""];
		if (entry.error) lines.push(theme.fg("warning", entry.error));
		const back = entry.input.getValue() ? "" : `  ${rawKeyHint("left", "back")}`;
		lines.push(entry.saving ? theme.fg("dim", "Saving key…") :
			`${keyHint("tui.input.submit", "save and select", { primaryOnly: true })}${back}  ${rawKeyHint("esc", "close")}`);
		return lines.map((line) => truncateToWidth(line, Math.max(1, width), "…"));
	}

	private async saveKey(entry: WebKeyEntry, value: string): Promise<void> {
		const actions = this.keyEntryActions;
		if (!actions || entry.saving || this.keyEntry !== entry) return;
		const key = value.trim();
		if (!/^[\x21-\x7e]{1,1024}$/.test(key) || key.startsWith("!")) {
			entry.error = "Enter a single API key.";
			return;
		}
		entry.saving = true;
		entry.error = undefined;
		try {
			const updated = await actions.save(entry.credentialId, key);
			if (this.keyEntry !== entry || !updated) return;
			entry.input.setValue("");
			this.keyEntry = undefined;
			this.updateStatus(updated);
			const choice = this.choices.find((candidate) => sameChoice(candidate.choice, entry.choice));
			if (choice?.keyStatus === "saved" && !choice.unavailable) {
				this.onSelect(choice.choice, entry.label);
			} else {
				this.onUnavailable(choice?.note ? `Key saved. ${choice.note}` : "Key saved, but the backend is not ready.");
			}
		} catch {
			// Avoid rendering a storage/RPC error that may contain the credential.
			if (this.keyEntry === entry) entry.error = "Couldn’t save the key. Try again.";
		} finally {
			entry.saving = false;
			actions.requestRender();
		}
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
		const credentialId = this.credentialToConfigure(row);
		if (credentialId && row.choice) {
			if (!this.keyEntryActions) return;
			const input = new Input();
			const entry: WebKeyEntry = { input, choice: row.choice, credentialId, label: singleLine(row.label), saving: false };
			input.onSubmit = (value) => { void this.saveKey(entry, value); };
			this.keyEntry = entry;
			return;
		}
		if (!row.choice || row.unavailable) {
			if (row.unavailable) this.onUnavailable(row.note ?? `${row.label} is unavailable.`);
			return;
		}
		if (row.choice.kind === "inherit" && row.active) {
			this.onCancel();
			return;
		}
		this.onSelect(row.choice, row.choice.kind === "inherit" ? "Default setup" : singleLine(row.label));
	}
}
