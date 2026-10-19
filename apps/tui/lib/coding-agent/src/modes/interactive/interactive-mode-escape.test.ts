import { test } from "node:test";
import assert from "node:assert/strict";

import { InteractiveMode } from "./interactive-mode.js";

test("Escape sends one session interrupt while retrying", async () => {
	for (const retryAttempt of [0, 1]) {
		let sessionInterrupts = 0;
		let normalAborts = 0;
		let retryAborts = 0;
		const sendSessionInterrupt = async () => {
			sessionInterrupts += 1;
		};
		const connection = {
			abort: async () => {
				normalAborts += 1;
				await sendSessionInterrupt();
			},
			// NativeAgentConnection.abortRetry() delegates to abort(), which sends
			// the same session/interrupt RPC.
			abortRetry: async () => {
				retryAborts += 1;
				await sendSessionInterrupt();
			},
			abortCompaction: async () => {},
			abortBranchSummary: async () => {},
			abortBash: async () => {},
		};
		const mode = Object.create(InteractiveMode.prototype) as {
			interruptOrClearInput(): void;
		};
		Object.assign(mode, {
			agentConnection: connection,
			connectionState: {
				retryAttempt,
				isCompacting: false,
				isBashRunning: false,
			},
		});

		mode.interruptOrClearInput();
		await Promise.resolve();
		assert.equal(sessionInterrupts, 1, `retryAttempt=${retryAttempt}`);
		assert.equal(normalAborts, retryAttempt === 0 ? 1 : 0);
		assert.equal(retryAborts, retryAttempt === 0 ? 0 : 1);
	}
});
