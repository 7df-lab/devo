import { type Component, Container, getKeybindings, Spacer, Text, truncateToWidth } from "@earendil-works/pi-tui";
import { theme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";
import { keyHint, keyText } from "./keybinding-hints.js";

interface UserMessageItem {
	id: string;
	text: string;
	timestamp?: string;
}

class UserMessageList implements Component {
	private messages: UserMessageItem[] = [];
	private selectedIndex: number = 0;
	public onSelect?: (entryId: string) => void;
	public onCancel?: () => void;
	private maxVisible: number = 10;

	constructor(messages: UserMessageItem[], initialSelectedId?: string) {
		// Session history is chronological; default to the latest fork point.
		this.messages = messages;
		const initialIndex = initialSelectedId ? messages.findIndex((message) => message.id === initialSelectedId) : -1;
		this.selectedIndex = initialIndex >= 0 ? initialIndex : Math.max(0, messages.length - 1);
	}

	setVisibleItems(count: number): void {
		this.maxVisible = Math.max(1, Math.min(10, count));
	}

	invalidate(): void {}

	render(width: number): string[] {
		const lines: string[] = [];

		if (this.messages.length === 0) {
			lines.push(theme.fg("muted", "  No user messages found"));
			return lines;
		}

		const startIndex = Math.max(
			0,
			Math.min(this.selectedIndex - Math.floor(this.maxVisible / 2), this.messages.length - this.maxVisible),
		);
		const endIndex = Math.min(startIndex + this.maxVisible, this.messages.length);

		for (let i = startIndex; i < endIndex; i++) {
			const message = this.messages[i];
			const isSelected = i === this.selectedIndex;

			const normalizedMessage = message.text.replace(/\n/g, " ").trim();

			const cursor = isSelected ? theme.fg("accent", "› ") : "  ";
			const maxMsgWidth = Math.max(0, width - 2);
			const truncatedMsg = truncateToWidth(normalizedMessage, maxMsgWidth);
			const messageLine = cursor + (isSelected ? theme.bold(truncatedMsg) : truncatedMsg);

			lines.push(messageLine);

			const position = i + 1;
			const metadata = `  Message ${position} of ${this.messages.length}`;
			const metadataLine = theme.fg("muted", metadata);
			lines.push(truncateToWidth(metadataLine, width));
			lines.push("");
		}

		if (startIndex > 0 || endIndex < this.messages.length) {
			const scrollInfo = theme.fg("muted", `  (${this.selectedIndex + 1}/${this.messages.length})`);
			lines.push(truncateToWidth(scrollInfo, width));
		}

		return lines;
	}

	handleInput(keyData: string): void {
		const kb = getKeybindings();
		if (kb.matches(keyData, "tui.select.up")) {
			this.selectedIndex = this.selectedIndex === 0 ? this.messages.length - 1 : this.selectedIndex - 1;
		} else if (kb.matches(keyData, "tui.select.down")) {
			this.selectedIndex = this.selectedIndex === this.messages.length - 1 ? 0 : this.selectedIndex + 1;
		} else if (kb.matches(keyData, "tui.select.confirm")) {
			const selected = this.messages[this.selectedIndex];
			if (selected && this.onSelect) {
				this.onSelect(selected.id);
			}
		} else if (kb.matches(keyData, "tui.select.cancel")) {
			if (this.onCancel) {
				this.onCancel();
			}
		}
	}
}

export class UserMessageSelectorComponent extends Container {
	private messageList: UserMessageList;
	private header = new Container();
	private footer = new Container();

	constructor(
		messages: UserMessageItem[],
		onSelect: (entryId: string) => void,
		onCancel: () => void,
		initialSelectedId?: string,
		private readonly getRows: () => number = () => process.stdout.rows || 24,
	) {
		super();

		this.header.addChild(new Spacer(1));
		this.header.addChild(new Text("Fork from Message", 1, 0));
		this.header.addChild(
			new Text(
				theme.fg("muted", "Select a message to copy the active path up to that point into a new session"),
				1,
				0,
			),
		);
		this.header.addChild(new Spacer(1));
		this.header.addChild(new DynamicBorder());
		this.header.addChild(new Spacer(1));

		this.messageList = new UserMessageList(messages, initialSelectedId);
		this.messageList.onSelect = onSelect;
		this.messageList.onCancel = onCancel;

		this.addChild(this.header);
		this.addChild(this.messageList);

		this.footer.addChild(new Text(
			theme.fg("dim", `${keyText("tui.select.up", { primaryOnly: true })}/${keyText("tui.select.down", { primaryOnly: true })}`)
				+ theme.fg("muted", " move  ")
				+ keyHint("tui.select.confirm", "fork", { primaryOnly: true }) + "  "
				+ keyHint("tui.select.cancel", "cancel", { primaryOnly: true }),
			1, 0,
		));
		this.footer.addChild(new DynamicBorder());
		this.addChild(this.footer);
	}

	render(width: number): string[] {
		const rows = this.getRows();
		const viewportRows = Number.isFinite(rows) && rows > 0 ? Math.floor(rows) : 24;
		// Leave four rows for the surrounding chat/editor chrome, plus the list counter.
		const reservedRows = this.header.render(width).length + this.footer.render(width).length + 5;
		this.messageList.setVisibleItems(Math.floor((viewportRows - reservedRows) / 3));
		return super.render(width);
	}

	getMessageList(): UserMessageList {
		return this.messageList;
	}
}
