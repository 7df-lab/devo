import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ProcessTerminal } from "@earendil-works/pi-tui";

describe("ProcessTerminal abnormal exit restore", () => {
	function withMockedTerminalIo<T>(run: (ctx: { writes: string[]; getIsRaw: () => boolean }) => T): T {
		const writes: string[] = [];
		let isRaw = false;

		const previousWrite = process.stdout.write;
		const previousSetRawMode = process.stdin.setRawMode;
		const previousIsRawDescriptor = Object.getOwnPropertyDescriptor(process.stdin, "isRaw");
		const previousPause = process.stdin.pause.bind(process.stdin);

		process.stdout.write = ((chunk: string | Uint8Array) => {
			writes.push(String(chunk));
			return true;
		}) as typeof process.stdout.write;

		Object.defineProperty(process.stdin, "isRaw", {
			configurable: true,
			get: () => isRaw,
		});
		process.stdin.setRawMode = ((value: boolean) => {
			isRaw = value;
			return process.stdin;
		}) as typeof process.stdin.setRawMode;

		try {
			return run({ writes, getIsRaw: () => isRaw });
		} finally {
			try {
				previousPause();
			} catch {
				/* ignore */
			}
			process.stdout.write = previousWrite;
			if (previousSetRawMode) {
				process.stdin.setRawMode = previousSetRawMode;
			} else {
				delete (process.stdin as { setRawMode?: typeof process.stdin.setRawMode }).setRawMode;
			}
			if (previousIsRawDescriptor) {
				Object.defineProperty(process.stdin, "isRaw", previousIsRawDescriptor);
			} else {
				delete (process.stdin as { isRaw?: boolean }).isRaw;
			}
		}
	}

	it("restores cooked mode on process exit without stop()", () => {
		withMockedTerminalIo(({ writes, getIsRaw }) => {
			const terminal = new ProcessTerminal();
			const rawModes: boolean[] = [];
			const previousSetRawMode = process.stdin.setRawMode!;
			process.stdin.setRawMode = ((value: boolean) => {
				rawModes.push(value);
				return previousSetRawMode(value);
			}) as typeof process.stdin.setRawMode;

			try {
				terminal.start(
					() => {},
					() => {},
				);
				assert.equal(getIsRaw(), true);
				assert.ok(writes.some((write) => write.includes("\x1b[?2004h")));

				process.emit("exit", 1);

				assert.equal(getIsRaw(), false);
				assert.ok(rawModes.includes(false));
				assert.ok(writes.some((write) => write.includes("\x1b[?2004l")));
				assert.ok(writes.some((write) => write.includes("\x1b[?25h")));
			} finally {
				try {
					terminal.stop();
				} catch {
					/* already restored */
				}
				process.stdin.setRawMode = previousSetRawMode;
			}
		});
	});

	it("stop() restores cooked mode and does not rely on a later exit event", () => {
		withMockedTerminalIo(({ getIsRaw }) => {
			const terminal = new ProcessTerminal();
			terminal.start(
				() => {},
				() => {},
			);
			assert.equal(getIsRaw(), true);
			terminal.stop();
			assert.equal(getIsRaw(), false);

			const previousSetRawMode = process.stdin.setRawMode!;
			let touched = false;
			process.stdin.setRawMode = ((value: boolean) => {
				touched = true;
				return previousSetRawMode(value);
			}) as typeof process.stdin.setRawMode;
			try {
				process.emit("exit", 0);
				assert.equal(touched, false);
				assert.equal(getIsRaw(), false);
			} finally {
				process.stdin.setRawMode = previousSetRawMode;
			}
		});
	});
});
