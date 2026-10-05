import { test } from "node:test";
import assert from "node:assert/strict";

import { StdinBuffer } from "@earendil-works/pi-tui";

function collect(buffer: StdinBuffer) {
	const sequences: string[] = [];
	const pastes: string[] = [];
	buffer.on("data", (chunk: string) => sequences.push(chunk));
	buffer.on("paste", (content: string) => pastes.push(content));
	return { sequences, pastes };
}

test("a coalesced fast-typing chunk keeps Enter as the submit key, not a paste", () => {
	// text + Enter key + text arriving in ONE stdin event: the bare \r is the
	// Enter key (terminals send CR alone), so it must split into key
	// sequences instead of becoming a literal newline via the paste path.
	const buffer = new StdinBuffer();
	const { sequences, pastes } = collect(buffer);

	buffer.process("CH1\rCH2");

	assert.deepStrictEqual(pastes, []);
	// Printable input is emitted per character; the \r stays its own key event
	// (the editor resolves it to submit), never paste content.
	assert.deepStrictEqual(sequences, [..."CH1", "\r", ..."CH2"]);
});

test("raw multiline clipboard text is still classified as a paste", () => {
	const buffer = new StdinBuffer();
	const { sequences, pastes } = collect(buffer);

	buffer.process("line one\nline two");

	assert.deepStrictEqual(pastes, ["line one\nline two"]);
	assert.deepStrictEqual(sequences, []);
});

test("crlf multiline text is still classified as a paste", () => {
	const buffer = new StdinBuffer();
	const { pastes } = collect(buffer);

	buffer.process("line one\r\nline two");

	assert.deepStrictEqual(pastes, ["line one\r\nline two"]);
});

test("a trailing Enter alone stays ordinary key input", () => {
	const buffer = new StdinBuffer();
	const { pastes } = collect(buffer);

	buffer.process("/tree\r");

	assert.deepStrictEqual(pastes, []);
});
