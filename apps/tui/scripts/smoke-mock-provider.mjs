/** Minimal streaming mock used by the CI tmux smoke (secret-free).
 *
 * Serves one deterministic assistant reply per POST so scenario `T2`
 * (send a prompt, assert a completed turn) runs without a real provider.
 * Wire is chosen by `--wire`:
 *   openai    → OpenAI-completions SSE chunks + `[DONE]` (default)
 *   anthropic → Anthropic Messages SSE event stream
 *   responses → OpenAI Responses SSE (`event:` + `data:` pairs; the official
 *               SDK parser rejects data-only frames)
 * Start with `node scripts/smoke-mock-provider.mjs [port] [--wire openai|anthropic|responses]`;
 * listens on 127.0.0.1 and never validates credentials.
 */
import { createServer } from "node:http";

const args = process.argv.slice(2);
const port = Number(args.find((a) => !a.startsWith("--")) ?? 8801);
const wire = args.includes("--wire") ? args[args.indexOf("--wire") + 1] : "openai";
const reply = "Smoke acknowledged.";

function openaiStream(response) {
	const chunks = [
		{
			id: "smoke",
			object: "chat.completion.chunk",
			created: 0,
			model: "deepseek-v4-flash",
			choices: [
				{ index: 0, delta: { role: "assistant", content: reply }, finish_reason: null },
			],
		},
		{
			id: "smoke",
			object: "chat.completion.chunk",
			created: 0,
			model: "deepseek-v4-flash",
			choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
		},
	];
	for (const chunk of chunks) response.write(`data: ${JSON.stringify(chunk)}\n\n`);
	response.end("data: [DONE]\n\n");
}

function anthropicStream(response) {
	const events = [
		["message_start", { type: "message_start", message: { id: "msg_smoke", type: "message", role: "assistant", model: "deepseek-v4-flash", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 1 } } }],
		["content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }],
		["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: reply } }],
		["content_block_stop", { type: "content_block_stop", index: 0 }],
		["message_delta", { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 2 } }],
		["message_stop", { type: "message_stop" }],
	];
	for (const [event, payload] of events) {
		response.write(`event: ${event}\n`);
		response.write(`data: ${JSON.stringify(payload)}\n\n`);
	}
	response.end();
}

function responsesStream(response) {
	const events = [
		{ type: "response.created", sequence_number: 0, response: { id: "resp_smoke", object: "response", status: "in_progress", model: "gpt-5-mini", output: [] } },
		{ type: "response.output_item.added", sequence_number: 1, output_index: 0, item: { id: "msg_1", type: "message", status: "in_progress", role: "assistant", content: [] } },
		{ type: "response.content_part.added", sequence_number: 2, item_id: "msg_1", output_index: 0, content_index: 0, part: { type: "output_text", text: "", annotations: [] } },
		{ type: "response.output_text.delta", sequence_number: 3, item_id: "msg_1", output_index: 0, content_index: 0, delta: reply },
		{ type: "response.output_text.done", sequence_number: 4, item_id: "msg_1", output_index: 0, content_index: 0, text: reply },
		{ type: "response.content_part.done", sequence_number: 5, item_id: "msg_1", output_index: 0, content_index: 0, part: { type: "output_text", text: reply, annotations: [] } },
		{ type: "response.output_item.done", sequence_number: 6, output_index: 0, item: { id: "msg_1", type: "message", status: "completed", role: "assistant", content: [{ type: "output_text", text: reply, annotations: [] }] } },
		{ type: "response.completed", sequence_number: 7, response: { id: "resp_smoke", object: "response", status: "completed", model: "gpt-5-mini", output: [{ id: "msg_1", type: "message", status: "completed", role: "assistant", content: [{ type: "output_text", text: reply, annotations: [] }] }], usage: { input_tokens: 1, input_tokens_details: { cached_tokens: 0 }, output_tokens: 2, output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 3 } } },
	];
	for (const event of events) {
		response.write(`event: ${event.type}\n`);
		response.write(`data: ${JSON.stringify(event)}\n\n`);
	}
	response.end();
}

createServer((_request, response) => {
	response.writeHead(200, { "Content-Type": "text/event-stream" });
	if (wire === "anthropic") anthropicStream(response);
	else if (wire === "responses") responsesStream(response);
	else openaiStream(response);
}).listen(port, "127.0.0.1", () => {
	console.log(`smoke mock provider (${wire}) on http://127.0.0.1:${port}`);
});
