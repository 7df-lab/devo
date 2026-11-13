/** Controlled HTTP provider, but real CLI / InteractiveMode / terminal resizing. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

export async function startResizeProvider(home) {
  const pending = new Map();
  const server = createServer(async (request, response) => {
    try {
      let raw = "";
      for await (const chunk of request) raw += chunk;
      const body = JSON.parse(raw);
      if (body.stream !== true) {
        // Title/background metadata calls must not steal the held turn response.
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ id: "resize-title", object: "chat.completion", choices: [{ index: 0,
          message: { role: "assistant", content: "Resize smoke" }, finish_reason: "stop" }] }));
        return;
      }
      const lastUser = body.messages?.filter(message => message.role === "user").at(-1);
      const match = JSON.stringify(lastUser?.content ?? "").match(/resize status test ([01])/);
      const turn = match ? Number(match[1]) : undefined;
      if (turn === undefined || pending.has(turn)) {
        response.writeHead(400, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ error: { message: "Unexpected or duplicate resize smoke turn" } }));
        return;
      }
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      response.write(`data: ${JSON.stringify({ id: `resize-${turn}`, choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }] })}\n\n`);
      pending.set(turn, response);
      response.on("close", () => { if (pending.get(turn) === response) pending.delete(turn); });
    } catch {
      response.writeHead(400, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ error: { message: "Invalid resize smoke request" } }));
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "ci-mock-key" } }), { mode: 0o600 });
  return {
    ready: turn => pending.has(turn),
    release(turn) {
      const response = pending.get(turn);
      if (!response) throw new Error(`No held response for resize turn ${turn}`);
      pending.delete(turn);
      response.end(`data: ${JSON.stringify({ id: "resize", choices: [{ index: 0, delta: { content: "Resize acknowledged." }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`);

    },
    close() { server.closeAllConnections(); server.close(); },
  };
}

export async function checkResizeStatus({ provider, mux, session, send, waitFor, records, turnOutcome }) {
  const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
  let probeId = 0;
  async function verifyFrame(label) {
    const probe = `resize-probe-${++probeId}`;
    mux(["send-keys", "-t", session, "-l", probe]);
    await waitFor(`${label} rendered input barrier`, () => screen().includes(probe));
    if (!status.test(screen())) throw new Error(`${label}: working status missing after rendered input`);
    mux(["send-keys", "-t", session, "C-u"]);
  }
  const status = /(?:Waiting|Thinking|Writing|Executing)\s*·\s*\d+s/;
  await send("/fullscreen on");
  for (let turn = 0; turn < 2; turn++) {
    const start = records().length;
    await send(`resize status test ${turn}`);
    await waitFor("held model turn and live status", () => provider.ready(turn) && status.test(screen()));
    for (const [width, height] of [[40, 12], [100, 30], [60, 16], [100, 30]]) {
      mux(["resize-window", "-t", session, "-x", String(width), "-y", String(height)]);
      await verifyFrame(`live status at ${width}x${height}`);
    }
    // Burst without waiting for a product frame between the two resizes.
    mux(["resize-window", "-t", session, "-x", "40", "-y", "8"]);
    mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
    await verifyFrame("live status after rapid round trip");
    if (turnOutcome(records().slice(start), "completed")) throw new Error("Resize test turn ended before provider release");
    provider.release(turn);
    await waitFor("completed turn", events => turnOutcome(events.slice(start), "completed"));
    await sleep(150);
    mux(["resize-window", "-t", session, "-x", "40", "-y", "12"]);
    mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
    await sleep(300);
    if (status.test(screen())) throw new Error("Resize resurrected a completed turn status");
  }
}
