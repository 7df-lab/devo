/** Real product TTY with a gated HTTP/SSE provider; no mocked UI or kernel. */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const repo = resolve(import.meta.dirname, "../../..");
const binary = resolve(process.env.DEVO_BIN || join(repo, "target/debug/devo"));
const home = mkdtempSync(join(tmpdir(), "devo-code-stream-"));
const session = `devo-code-stream-${process.pid}`;
const trace = join(home, "trace.jsonl");
const timeout = Number(process.env.DEVO_TUI_TEST_TIMEOUT_MS || 90000);
let pane = "";
let response;
let requestCount = 0;
const chunk = delta => response.write(`data: ${JSON.stringify({ id: "code-stream", object: "chat.completion.chunk", model: "deepseek-v4-flash", choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`);
const server = createServer(async (req, res) => {
  for await (const _ of req) { /* drain request */ }
  requestCount++;
  response = res;
  res.writeHead(200, { "Content-Type": "text/event-stream" });
  res.flushHeaders();
  chunk({ role: "assistant", tool_calls: [{ index: 0, id: `call-${requestCount}`, type: "function", function: { name: "ipython", arguments: "" } }] });
});
const mux = args => {
  const result = spawnSync(process.env.DEVO_TMUX_BIN || "tmux", args, { encoding: "utf8", timeout: 10000 });
  if (result.status !== 0) throw new Error(result.stderr || String(result.error));
  return result.stdout;
};
const records = () => existsSync(trace) ? readFileSync(trace, "utf8").split("\n").slice(0, -1).filter(Boolean).map(JSON.parse) : [];
async function waitFor(label, test) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    pane = mux(["capture-pane", "-t", session, "-p"]);
    if (test(pane, records())) return;
    await sleep(100);
  }
  throw new Error(`Timed out: ${label}\n${pane}`);
}
async function send(text) {
  mux(["send-keys", "-t", session, "-l", text]);
  mux(["send-keys", "-t", session, "Enter"]);
}
try {
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: { credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${port}`, wire_api: "openai_chat_completions" } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "local-fixture-only" } }), { mode: 0o600 });
  writeFileSync(join(home, "settings.json"), JSON.stringify({ onboardingShown: true }));
  mux(["new-session", "-d", "-s", session, "-c", repo, "-x", "100", "-y", "30", "-e", `DEVO_HOME=${home}`, "-e", `DEVO_CODING_AGENT_DIR=${home}`, "-e", `PRIME_AGENT_CODING_AGENT_DIR=${home}`, "-e", "DEVO_PROTOCOL_TRACE=1", "-e", `DEVO_PROTOCOL_TRACE_FILE=${trace}`, "--", binary]);
  await waitFor("trust", text => text.includes("Trust this workspace?"));
  await send("yes");
  await waitFor("default Details", text => text.includes("Details") && text.includes("Scheduled"));
  for (const width of [100, 40]) {
    mux(["resize-window", "-t", session, "-x", String(width), "-y", "30"]);
    const before = requestCount;
    const traceStart = records().length;
    await send("Show a small Python cell");
    await waitFor("model request", () => requestCount > before);
    const turnId = records().slice(traceStart).find(row => row.method === "turn/started")?.payload?.params?.turn?.id;
    assert.ok(turnId, "current turn must start before argument deltas");
    // Argument-only deltas do not feed the activity tracker. Exercise the
    // working count with a real assistant text delta before streaming code.
    chunk({ content: "Preparing the requested Python example." });
    await waitFor("live token count without estimate suffix", text => /↓ \d+ tokens/.test(text));
    assert.ok(!pane.includes("(est.)"), "working token label must not include the estimate suffix");
    chunk({ tool_calls: [{ index: 0, function: { arguments: '{"code":"stream_first = 1' } }] });
    await waitFor("first partial source", text => text.includes("╰─ stream_first = 1"));
    assert.ok(!pane.includes("stream_second"));
    chunk({ tool_calls: [{ index: 0, function: { arguments: '\\nstream_second = 2' } }] });
    await waitFor("second partial source before closing JSON", text => text.includes("stream_second = 2"));
    assert.ok(!records().slice(traceStart).some(row => row.method === "tool_call/status_updated" &&
      row.payload?.params?.toolCallId === `call-${requestCount}` && row.payload.params.status === "in_progress"), "must not execute incomplete code");
    console.log(`Python argument stream visible at ${width} columns before JSON completion:\n${pane}`);
    mux(["send-keys", "-t", session, "Escape"]);
    await waitFor("abort closes pending cell", (text, events) => events.slice(traceStart).some(row => row.method === "turn/completed" && row.payload?.params?.turn?.id === turnId && row.payload.params.turn.status === "interrupted") && !text.includes("╰─ stream_first = 1"));
    response.destroy();
  }
  // Manual cycling stays available and leaves the selected mode intact.
  for (const expected of ["Expanded", "Collapsed", "Details"]) {
    mux(["send-keys", "-t", session, "C-o"]);
    await waitFor(`manual ${expected}`, text => text.includes(expected));
  }
  const savedSession = records().find(row => row.method === "turn/started")?.payload?.params?.turn?.sessionId;
  assert.ok(savedSession, "created Native session id");
  const restoreStart = records().length;
  await send(`/resume ${savedSession}`);
  await waitFor("history reload keeps Details", (text, events) => {
    const current = events.slice(restoreStart);
    const requests = new Set(current.filter(row => row.direction === "tui-to-server" && row.method === "session/resume").map(row => row.id));
    return current.some(row => row.kind === "response" && requests.has(row.id) && row.payload?.result && !row.payload.error)
      && text.includes("Resumed session") && text.includes("Details") && !text.includes("╰─ stream_first = 1");
  });
  console.log("python-code-stream passed");
} finally {
  console.log(pane);
  spawnSync(process.env.DEVO_TMUX_BIN || "tmux", ["kill-session", "-t", session]);
  response?.destroy();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
  rmSync(home, { recursive: true, force: true });
}
