/** Controlled HTTP provider, but real CLI / InteractiveMode / terminal resizing. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

export async function startResizeProvider(home) {
  let pending;
  const server = createServer((request, response) => {
    request.resume();
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    response.write(`data: ${JSON.stringify({ id: "resize", choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }] })}\n\n`);
    pending = response;
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "ci-mock-key" } }), { mode: 0o600 });
  return {
    ready: () => Boolean(pending),
    release() {
      pending.end(`data: ${JSON.stringify({ id: "resize", choices: [{ index: 0, delta: { content: "Resize acknowledged." }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`);
      pending = undefined;
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
    await waitFor("held model turn and live status", () => provider.ready() && status.test(screen()));
    for (const [width, height] of [[40, 12], [100, 30], [60, 16], [100, 30]]) {
      mux(["resize-window", "-t", session, "-x", String(width), "-y", String(height)]);
      await verifyFrame(`live status at ${width}x${height}`);
    }
    // Burst without waiting for a product frame between the two resizes.
    mux(["resize-window", "-t", session, "-x", "40", "-y", "8"]);
    mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
    await verifyFrame("live status after rapid round trip");
    if (turnOutcome(records().slice(start), "completed")) throw new Error("Resize test turn ended before provider release");
    provider.release();
    await waitFor("completed turn", events => turnOutcome(events.slice(start), "completed"));
    await sleep(150);
    mux(["resize-window", "-t", session, "-x", "40", "-y", "12"]);
    mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
    await sleep(300);
    if (status.test(screen())) throw new Error("Resize resurrected a completed turn status");
  }
}
