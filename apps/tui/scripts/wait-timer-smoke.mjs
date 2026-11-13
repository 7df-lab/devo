/** Real product session navigation while the second model request is held open. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

export async function startWaitTimerProvider(home) {
  const pending = new Map();
  const calls = new Map();
  const server = createServer(async (request, response) => {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    const body = JSON.parse(raw);
    if (body.stream !== true) {
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ id: "title", object: "chat.completion", choices: [{ index: 0,
        message: { role: "assistant", content: "Wait timer smoke" }, finish_reason: "stop" }] }));
      return;
    }
    const lastUser = body.messages?.filter(message => message.role === "user").at(-1);
    const match = JSON.stringify(lastUser?.content ?? "").match(/wait timer test ([01])/);
    const turn = match ? Number(match[1]) : undefined;
    const count = (calls.get(turn) ?? 0) + 1;
    if (turn === undefined || count > 2) {
      response.writeHead(400); response.end("Unexpected wait timer request"); return;
    }
    calls.set(turn, count);
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    const chunk = (delta, finish_reason = null) => `data: ${JSON.stringify({ id: `wait-${turn}`, choices: [{ index: 0, delta, finish_reason }] })}\n\n`;
    if (count === 1) {
      // A completed model message inside a still-running turn defeats the old
      // transcript-backscan clock. The following tool result starts request 2.
      response.end(chunk({ role: "assistant", content: "Checking the clock.", tool_calls: [{ index: 0,
        id: `clock-${turn}`, type: "function", function: { name: "ipython", arguments: JSON.stringify({ code: "print('clock ready')" }) } }] }, "tool_calls") + "data: [DONE]\n\n");
    } else {
      response.write(chunk({ role: "assistant" }));
      pending.set(turn, () => response.end(chunk({ content: "Clock checked." }, "stop") + "data: [DONE]\n\n"));
      response.on("close", () => pending.delete(turn));
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "ci-mock-key" } }), { mode: 0o600 });
  return { ready: turn => pending.has(turn), calls,
    release(turn) { const release = pending.get(turn); if (!release) throw new Error("Missing held response"); release(); },
    close() { server.closeAllConnections(); server.close(); } };
}

export async function checkWaitTimer({ provider, mux, session, send, waitFor, records, turnOutcome }) {
  const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
  const elapsed = () => {
    const match = screen().match(/(?:Waiting|Thinking|Writing|Executing)\s*·\s*(?:(\d+)m\s*)?(\d+)s/);
    return match ? Number(match[1] ?? 0) * 60 + Number(match[2]) : undefined;
  };
  await send("/fullscreen on");
  for (const turn of [0, 1]) {
    mux(["resize-window", "-t", session, "-x", turn === 0 ? "100" : "40", "-y", "30"]);
    const start = records().length;
    await send(`wait timer test ${turn}`);
    await waitFor("second model request held with elapsed clock", () => provider.ready(turn) && elapsed() >= 3);
    const before = elapsed();
    mux(["send-keys", "-t", session, "Left"]);
    await waitFor("session manager", () => screen().includes("Search sessions"));
    await sleep(1200);
    mux(["send-keys", "-t", session, "Enter"]);
    await waitFor("running session reattached", () => !screen().includes("Search sessions") && elapsed() !== undefined);
    // A real key/render barrier rules out accepting the pre-navigation frame.
    const probe = `clock-probe-${turn}`;
    mux(["send-keys", "-t", session, "-l", probe]);
    await waitFor("reattached editor rendered", () => screen().includes(probe));
    if (elapsed() < before) throw new Error(`Elapsed clock reset: ${before}s -> ${elapsed()}s`);
    mux(["send-keys", "-t", session, "C-u"]);
    if (provider.calls.get(turn) !== 2 || turnOutcome(records().slice(start), "completed")) throw new Error("Navigation restarted or ended the held turn");
    console.log(`=== wait timer reattached ${turn} ===\n${screen()}`);
    provider.release(turn);
    await waitFor("same turn finishes", events => turnOutcome(events.slice(start), "completed"));
    await waitFor("completed clock removed", () => elapsed() === undefined);
  }
}
