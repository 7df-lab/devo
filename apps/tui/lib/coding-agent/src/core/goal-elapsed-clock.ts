import type { GoalState } from "./goals.js";

/** Display-only active pursuit estimate. Server turn accounting remains authoritative. */
export class GoalElapsedClock {
	private key: string | undefined;
	private seconds = 0;
	private sampledAt = 0;
	private active = false;
	private connected = true;

	constructor(private readonly now: () => number = () => performance.now()) {}

	setConnected(connected: boolean): void {
		const now = this.now();
		if (this.active && this.connected) this.seconds += Math.max(0, now - this.sampledAt) / 1000;
		this.sampledAt = now;
		this.connected = connected;
	}

	read(goal: GoalState, sessionId: string): number {
		const now = this.now();
		const key = JSON.stringify([sessionId, goal.goalId]);
		const accounted = Number.isFinite(goal.timeUsedSeconds) ? Math.max(0, goal.timeUsedSeconds) : 0;
		if (this.key !== key || goal.status === "idle") {
			this.key = key;
			this.seconds = accounted;
		} else {
			if (this.active && this.connected) this.seconds += Math.max(0, now - this.sampledAt) / 1000;
			// A settled turn already includes time estimated locally: never add it twice.
			this.seconds = Math.max(this.seconds, accounted);
		}
		this.sampledAt = now;
		this.active = goal.status === "active";
		return this.seconds;
	}
}
