/**
 * Bug: concurrent modal extension dialogs (approval select/confirm, input,
 * editor) all render into the same editor container. The second dialog
 * detached the first component whose settle callbacks could never fire
 * again, so an approval answer was never sent and the requesting subagent
 * turn hung at its approval checkpoint forever. The queue serializes the
 * dialogs instead.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { ExtensionDialogQueue } from "../lib/coding-agent/src/modes/interactive/components/extension-dialog-queue.js";

test("extension dialog queue runs the first presentation immediately and queues the rest", () => {
	const queue = new ExtensionDialogQueue();
	const order: string[] = [];

	queue.run(() => order.push("a shown"));
	queue.run(() => order.push("b shown"));
	queue.run(() => order.push("c shown"));

	assert.deepEqual(order, ["a shown"]);
	assert.equal(queue.pendingCount, 2);

	queue.release();
	assert.deepEqual(order, ["a shown", "b shown"]);

	queue.release();
	assert.deepEqual(order, ["a shown", "b shown", "c shown"]);
	assert.equal(queue.pendingCount, 0);

	queue.release();
	assert.deepEqual(order, ["a shown", "b shown", "c shown"]);
});

test("extension dialog queue lets a declined presentation release the lane itself", () => {
	const queue = new ExtensionDialogQueue();
	const order: string[] = [];

	queue.run(() => order.push("a shown"));
	// Declined (e.g. request aborted while queued): must free the lane so the
	// next queued dialog still gets presented.
	queue.run(() => {
		order.push("b declined");
		queue.release();
	});
	queue.run(() => order.push("c shown"));

	queue.release();
	assert.deepEqual(order, ["a shown", "b declined", "c shown"]);
});

test("extension dialog queue ignores release when idle", () => {
	const queue = new ExtensionDialogQueue();
	const order: string[] = [];

	queue.run(() => order.push("a"));
	queue.release();
	// Spurious release after the lane is already free must not flip state.
	queue.release();
	assert.deepEqual(order, ["a"]);

	queue.run(() => order.push("b"));
	queue.release();
	assert.deepEqual(order, ["a", "b"]);
});

test("extension dialog queue drains synchronously when presentations decline", () => {
	const queue = new ExtensionDialogQueue();
	const order: string[] = [];

	queue.run(() => order.push("a"));
	for (const label of ["b", "c", "d"]) {
		queue.run(() => {
			order.push(`${label} declined`);
			queue.release();
		});
	}

	queue.release();
	assert.deepEqual(order, ["a", "b declined", "c declined", "d declined"]);
	assert.equal(queue.pendingCount, 0);
});
