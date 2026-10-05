/**
 * Serializes modal extension dialogs (select, confirm, input, editor).
 *
 * All modal extension dialogs render into the same editor container, so
 * presenting a second dialog while one is open detaches the open component:
 * its settle callbacks can never fire again, and any caller awaiting it —
 * e.g. an approval answer that must be sent back over the connection —
 * hangs forever. Presentations taken while a dialog is active are held here
 * until the active one is dismissed.
 */
export class ExtensionDialogQueue {
	private readonly queued: Array<() => void> = [];
	private active = false;

	/** Number of presentations waiting for the active dialog to settle. */
	get pendingCount(): number {
		return this.queued.length;
	}

	/**
	 * Run `present` immediately when no dialog is active, or hold it until
	 * `release` frees the lane. `present` must either show a dialog (whose
	 * dismissal will call `release`) or call `release` itself when it decides
	 * not to show one.
	 */
	run(present: () => void): void {
		if (this.active) {
			this.queued.push(present);
			return;
		}
		this.active = true;
		present();
	}

	/**
	 * Mark the active dialog as settled and start the next queued
	 * presentation, if any. Idempotent: extra releases (e.g. a hide fired
	 * after the dialog already settled) are ignored so they cannot skip the
	 * queue's turn order.
	 */
	release(): void {
		if (!this.active) {
			return;
		}
		const next = this.queued.shift();
		if (next) {
			next();
			return;
		}
		this.active = false;
	}
}
