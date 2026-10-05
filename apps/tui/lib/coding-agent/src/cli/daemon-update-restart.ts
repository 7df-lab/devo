export function buildDaemonUpdateRestartReport(): string { return ""; }
export function launchDaemonUpdateRestartCoordinator(): never {
	throw new Error("Devo TUI does not self-update");
}
export function resolveDaemonUpdateRestartSocketPath(): undefined { return undefined; }
