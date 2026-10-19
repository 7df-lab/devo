/**
 * Counting semaphore for bounding async concurrency. FIFO: waiters acquire in
 * the order they queued.
 */
export class Semaphore {
	private available: number;
	private readonly waiters: Array<() => void> = [];

	constructor(permits: number) {
		if (!Number.isInteger(permits) || permits < 1) {
			throw new Error(`Semaphore permits must be a positive integer, got ${permits}`);
		}
		this.available = permits;
	}

	get queueLength(): number {
		return this.waiters.length;
	}

	private async acquire(signal?: AbortSignal): Promise<void> {
		if (signal?.aborted) {
			throw signal.reason ?? new Error("aborted");
		}
		if (this.available > 0) {
			this.available -= 1;
			return;
		}
		// A queued waiter can be aborted before it gets a permit; on abort it
		// removes itself from the queue so it never consumes a slot.
		await new Promise<void>((resolve, reject) => {
			const waiter = (): void => {
				signal?.removeEventListener("abort", onAbort);
				resolve();
			};
			const onAbort = (): void => {
				const i = this.waiters.indexOf(waiter);
				if (i !== -1) this.waiters.splice(i, 1);
				reject(signal?.reason ?? new Error("aborted"));
			};
			this.waiters.push(waiter);
			signal?.addEventListener("abort", onAbort, { once: true });
		});
	}

	private release(): void {
		const next = this.waiters.shift();
		if (next) {
			next();
		} else {
			this.available += 1;
		}
	}

	/** Run `fn` while holding a permit, releasing it even if `fn` throws. Rejects without running if `signal` aborts while queued. */
	async run<T>(fn: () => Promise<T>, signal?: AbortSignal): Promise<T> {
		await this.acquire(signal);
		try {
			return await fn();
		} finally {
			this.release();
		}
	}
}
