import type { AgentConnectionQueueState } from "../agent-connection/index.js";

export type QueueLane = "steering" | "followUp";

export interface QueueSelectionItem {
	id?: string;
	lane: QueueLane;
	index: number;
	text: string;
}

/**
 * Tracks which queued message the user is browsing/editing with alt+up/alt+down.
 *
 * Native entries retain stable IDs and admission order. Lane/index/text are
 * compatibility selectors only. Browsing is newest-first within that order;
 * older backends without ordered entries retain their legacy lane projection.
 */
export class QueueSelection {
	private items: QueueSelectionItem[] = [];
	private cursor = -1; // -1 = draft
	private draft = "";
	private hasStashedDraft = false;

	get selected(): QueueSelectionItem | undefined {
		return this.cursor >= 0 ? this.items[this.cursor] : undefined;
	}

	get isBrowsing(): boolean {
		return this.cursor >= 0;
	}

	get hasDraft(): boolean {
		return this.hasStashedDraft;
	}

	replaceDraft(draft: string): void {
		this.draft = draft;
		this.hasStashedDraft = true;
	}

	/** Move the cursor. -1 browses older, +1 newer. Returns the text to show, or undefined for a boundary noop. */
	move(queue: AgentConnectionQueueState, draft: string, direction: -1 | 1): string | undefined {
		if (this.cursor < 0) {
			if (direction > 0) return undefined;
			this.items = flatten(queue);
			if (this.items.length === 0) return undefined;
			if (!this.hasStashedDraft) {
				this.draft = draft;
				this.hasStashedDraft = true;
			}
			this.cursor = this.items.length - 1;
			return this.items[this.cursor]?.text;
		}
		const next = this.cursor + direction;
		if (next < 0 || next > this.items.length) return undefined;
		if (next === this.items.length) {
			return this.reset();
		}
		this.cursor = next;
		return this.items[next]?.text;
	}

	refreshAt(
		queue: AgentConnectionQueueState,
		lane: QueueLane,
		index: number,
		expectedText: string,
	): string | undefined {
		const selectedId = this.selected?.id;
		this.items = flatten(queue);
		const cursor = this.items.findIndex((item) => item.lane === lane && item.index === index);
		const selected = this.items[cursor];
		if (selected?.lane !== lane || selected.index !== index || selected.text !== expectedText || (selectedId && selected.id !== selectedId)) return this.reset();
		this.cursor = cursor;
	}

	/** Called after a mutation or submit resolved the selection. Returns the stashed draft. */
	reset(): string {
		this.cursor = -1;
		const draft = this.draft;
		this.draft = "";
		this.hasStashedDraft = false;
		return draft;
	}
}

function flatten(queue: AgentConnectionQueueState): QueueSelectionItem[] {
	if (queue.ordered) {
		const indices = { steering: 0, followUp: 0 };
		return queue.ordered.map((item) => {
			const lane = item.delivery === "steer" ? "steering" : "followUp";
			return { id: item.id, lane, index: indices[lane]++, text: item.text };
		});
	}
	return [
		...queue.steering.map((text, index) => ({ lane: "steering" as const, index, text })),
		...queue.followUp.map((text, index) => ({ lane: "followUp" as const, index, text })),
	];
}
