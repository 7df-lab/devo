/**
 * Lightweight tagged logger for the renderer process.
 *
 * All output is prefixed with the module name so you can filter
 * in DevTools console (e.g. filter by "[renderer:use-server]").
 *
 * Usage:
 *   const log = createLogger("use-server")
 *   log.info("Sending prompt", { sessionId })
 *   log.error("Prompt failed", err)
 */

export interface Logger {
	debug: (...args: unknown[]) => void
	info: (...args: unknown[]) => void
	warn: (...args: unknown[]) => void
	error: (...args: unknown[]) => void
}

export function createLogger(module: string): Logger {
	const tag = `[renderer:${module}]`
	return {
		debug: (...args: unknown[]) => console.debug(tag, ...args),
		info: (...args: unknown[]) => console.log(tag, ...args),
		warn: (...args: unknown[]) => console.warn(tag, ...args),
		error: (...args: unknown[]) => console.error(tag, ...args),
	}
}
