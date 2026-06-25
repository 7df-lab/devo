/**
 * Simple semaphore for limiting concurrent automation runs.
 */

export class Semaphore {
	private current = 0
	private readonly queue: (() => void)[] = []

	constructor(private readonly limit: number) {}

	async acquire(): Promise<() => void> {
		if (this.current < this.limit) {
			this.current++
			return () => this.release()
		}

		return new Promise<() => void>((resolve) => {
			this.queue.push(() => {
				this.current++
				resolve(() => this.release())
			})
		})
	}

	private release(): void {
		this.current--
		const next = this.queue.shift()
		if (next) next()
	}

	get pending(): number {
		return this.queue.length
	}

	get active(): number {
		return this.current
	}
}
