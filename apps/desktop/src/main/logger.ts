/**
 * Lightweight tagged logger for the Electron main process.
 *
 * Usage:
 *   const log = createLogger("devo-manager")
 *   log.info("Server started", { url, pid })
 *   log.error("Spawn failed", err)
 */

export interface Logger {
	debug: (...args: unknown[]) => void
	info: (...args: unknown[]) => void
	warn: (...args: unknown[]) => void
	error: (...args: unknown[]) => void
}

export function createLogger(module: string): Logger {
	const tag = `[main:${module}]`
	return {
		debug: (...args: unknown[]) => console.debug(tag, ...args),
		info: (...args: unknown[]) => console.log(tag, ...args),
		warn: (...args: unknown[]) => console.warn(tag, ...args),
		error: (...args: unknown[]) => console.error(tag, ...args),
	}
}
