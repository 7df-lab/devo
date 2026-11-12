/** Orders composer goal replies within one session/project lifecycle.
 * Mutations invalidate older reads and block polling until they settle.
 */
export class ComposerGoalRequests {
	private active = false
	private revision = 0
	private mutating = false
	private reading = false

	activate() {
		this.active = true
	}

	dispose() {
		this.active = false
		this.mutating = false
		this.reading = false
		this.revision++
	}

	read() {
		if (!this.active || this.mutating || this.reading) return null
		this.reading = true
		const revision = ++this.revision
		return {
			isCurrent: () => this.active && revision === this.revision,
			finish: () => {
				if (revision === this.revision) this.reading = false
			},
		}
	}

	mutate() {
		if (!this.active || this.mutating) return null
		this.mutating = true
		this.reading = false
		const revision = ++this.revision
		return {
			isCurrent: () => this.active && revision === this.revision,
			finish: () => {
				if (revision === this.revision) this.mutating = false
			},
		}
	}
}
