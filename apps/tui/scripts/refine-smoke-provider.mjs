import { createServer } from "node:http";

export async function createRefineSmokeProvider() {
  let calls = 0;
  let held;
  const server = createServer(async (request, response) => {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    const body = JSON.parse(raw || "{}");
    const refine = body.messages?.some(m => m.role === "system" && String(m.content).includes("Continual Harness"));
    if (refine) {
      calls++;
      if (calls === 2) { response.writeHead(400); response.end('{"error":{"message":"refine smoke failure"}}'); return; }
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ id: "refine", object: "chat.completion", created: 0, model: "smoke", choices: [{ index: 0, message: { role: "assistant", content: JSON.stringify({ summary: "Harness already clean", edits: [] }) }, finish_reason: "stop" }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } }));
    } else if (body.stream) {
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (delta, finish_reason) => response.write(`data: ${JSON.stringify({ id: "turn", object: "chat.completion.chunk", created: 0, model: "smoke", choices: [{ index: 0, delta, finish_reason }] })}\n\n`);
      send({ role: "assistant", content: "Working on refinement smoke." }, null);
      held = () => { send({}, "stop"); response.end("data: [DONE]\n\n"); held = undefined; };
    } else {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ choices: [{ message: { role: "assistant", content: "Refine smoke" }, finish_reason: "stop" }] }));
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { port: server.address().port, get calls() { return calls; }, get turnStarted() { return Boolean(held); }, releaseTurn() { held?.(); }, close() { server.closeAllConnections(); server.close(); } };
}
