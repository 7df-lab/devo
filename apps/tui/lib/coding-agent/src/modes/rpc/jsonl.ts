import type { Readable } from "node:stream";
import { StringDecoder } from "node:string_decoder";

/**
 * Serialize a single strict JSONL record.
 *
 * Framing is LF-only. Payload strings may contain other Unicode separators such as
 * U+2028 and U+2029. Clients must split records on `\n` only.
 */
export function serializeJsonLine(value: unknown): string {
	return `${JSON.stringify(value)}\n`;
}

export interface JsonlLineReaderOptions {
	maxLineLength?: number;
	onLineOverflow?: (prefix: string) => void;
}

/**
 * Attach an LF-only JSONL reader to a stream.
 *
 * This intentionally does not use Node readline. Readline splits on additional
 * Unicode separators that are valid inside JSON strings and therefore does not
 * implement strict JSONL framing.
 */
export function attachJsonlLineReader(
	stream: Readable,
	onLine: (line: string) => void,
	options: JsonlLineReaderOptions = {},
): () => void {
	const decoder = new StringDecoder("utf8");
	// Segments of the current, not-yet-terminated line. We never concatenate into a
	// single growing buffer: appending to one accumulator and rescanning it with
	// indexOf("\n") on every chunk is O(n^2) when a single record is split across
	// many socket reads (e.g. a multi-MB attach snapshot arriving in 64KB chunks).
	// Instead each chunk is scanned once (offset-advancing indexOf) and the segments
	// are joined exactly once, when the terminating newline arrives.
	let pending: string[] = [];
	let pendingLength = 0;
	let discardingOverflow = false;

	const emitLine = (line: string) => {
		onLine(line.endsWith("\r") ? line.slice(0, -1) : line);
	};

	const resetPending = () => {
		pending = [];
		pendingLength = 0;
	};

	const appendPending = (segment: string) => {
		if (discardingOverflow || segment.length === 0) return;
		const maxLineLength = options.maxLineLength;
		if (maxLineLength !== undefined && pendingLength + segment.length > maxLineLength) {
			const remaining = Math.max(0, maxLineLength - pendingLength);
			if (remaining > 0) {
				pending.push(segment.slice(0, remaining));
			}
			options.onLineOverflow?.(pending.join(""));
			resetPending();
			discardingOverflow = true;
			return;
		}
		pending.push(segment);
		pendingLength += segment.length;
	};

	const emitFrom = (segment: string) => {
		if (discardingOverflow) {
			discardingOverflow = false;
			resetPending();
			return;
		}
		appendPending(segment);
		if (discardingOverflow) {
			discardingOverflow = false;
			return;
		}
		emitLine(pending.join(""));
		resetPending();
	};

	const onData = (chunk: string | Buffer) => {
		const text = typeof chunk === "string" ? chunk : decoder.write(chunk);

		let start = 0;
		let newlineIndex = text.indexOf("\n");
		while (newlineIndex !== -1) {
			emitFrom(text.slice(start, newlineIndex));
			start = newlineIndex + 1;
			newlineIndex = text.indexOf("\n", start);
		}
		if (start < text.length) {
			appendPending(text.slice(start));
		}
	};

	const onEnd = () => {
		const tail = decoder.end();
		if (tail.length > 0) {
			appendPending(tail);
		}
		if (!discardingOverflow && pending.length > 0) {
			emitLine(pending.join(""));
		}
		resetPending();
		discardingOverflow = false;
	};

	stream.on("data", onData);
	stream.on("end", onEnd);

	return () => {
		stream.off("data", onData);
		stream.off("end", onEnd);
	};
}
