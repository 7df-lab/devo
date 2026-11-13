/** Genuine streamed tool-input refresh + queued admission, through product TTY. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

export async function startQueueOutputProvider(home) {
  const pending = new Map();
  const started = new Set();
  const errors = [];
  const server = createServer(async (request, response) => {
    try {
      let raw = "";
      for await (const chunk of request) raw += chunk;
      const body = JSON.parse(raw);
      if (body.stream !== true) {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ id: "title", choices: [{ index: 0,
          message: { role: "assistant", content: "Queue output check" }, finish_reason: "stop" }] }));
        return;
      }
      const users = body.messages?.filter(message => message.role === "user") ?? [];
      const match = JSON.stringify(users.at(-1)?.content ?? "").match(/queue-output (initial|queued) (steer|follow)/);
      if (!match) throw new Error("Unrecognized conversation request");
      const [, stage, mode] = match;
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      const chunk = (delta, finish_reason = null) => response.write(`data: ${JSON.stringify({
        id: `queue-${mode}`, choices: [{ index: 0, delta, finish_reason }],
      })}\n\n`);
      const done = () => { chunk({}, "stop"); response.end("data: [DONE]\n\n"); };
      if (stage === "initial" && !started.has(mode)) {
        started.add(mode);
        // The real provider emits early ToolUseStart, followed by prose. Its
        // assembled full-input ToolUseStart used to close the prose only in IM.
        chunk({ role: "assistant", tool_calls: [{ index: 0, id: `tool-${mode}`, type: "function",
          function: { name: "ipython", arguments: "" } }] });
        chunk({ content: `OLD_OUTPUT_${mode.toUpperCase()}` });
        pending.set(mode, () => {
          chunk({ tool_calls: [{ index: 0, function: { arguments: JSON.stringify({ code: "print('tool finished')" }) } }] });
          chunk({}, "tool_calls"); response.end("data: [DONE]\n\n");
        });
        response.on("close", () => pending.delete(mode));
      } else {
        chunk({ role: "assistant", content: stage === "queued" ? `QUEUED_REPLY_${mode.toUpperCase()}` : "Initial tool finished." });
        done();
      }
    } catch (error) {
      errors.push(String(error));
      if (!response.headersSent) response.writeHead(400);
      response.end(String(error));
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "ci-mock-key" } }), { mode: 0o600 });
  return { errors, ready: mode => pending.has(mode),
    release(mode) { const finish = pending.get(mode); if (!finish) throw new Error("Missing held stream"); finish(); },
    close() { server.closeAllConnections(); server.close(); } };
}

export function queuedTurnCompleted(events, mode, initialTurnId) {
  const queuedText = `queue-output queued ${mode}`;
  const admissions = events.filter(event => event.method === "item/started"
    && event.payload?.params?.item?.item?.type === "userMessage"
    && event.payload.params.item.item.content?.some(block => block.type === "text" && block.text === queuedText));
  if (admissions.length !== 1) return false;
  const admission = admissions[0].payload.params.item;
  if (!admission.turnId || !initialTurnId) return false;
  if ((admission.turnId === initialTurnId) !== (mode === "steer")) return false;
  const replies = events.filter(event => event.method === "item/completed"
    && event.payload?.params?.item?.item?.type === "assistantMessage"
    && event.payload.params.item.item.text === `QUEUED_REPLY_${mode.toUpperCase()}`);
  if (replies.length !== 1) return false;
  const reply = replies[0].payload.params.item;
  if (reply.turnId !== admission.turnId || reply.sessionId !== admission.sessionId || reply.state !== "completed") return false;
  const terminalIndex = events.findIndex(event => event.method === "turn/completed"
    && event.payload?.params?.turn?.id === admission.turnId
    && event.payload.params.turn.sessionId === admission.sessionId && event.payload.params.turn.status === "completed");
  return terminalIndex > events.indexOf(replies[0]) && events.indexOf(replies[0]) > events.indexOf(admissions[0]);
}

export async function checkQueueOutput({ provider, mux, session, send, waitFor, records, rpcSucceeded }) {
  await send("/fullscreen on");
  mux(["resize-window", "-t", session, "-x", "100", "-y", "50"]);
  for (const mode of ["follow", "steer"]) {
    const start = records().length;
    await send(`queue-output initial ${mode}`);
    const marker = `OLD_OUTPUT_${mode.toUpperCase()}`;
    await waitFor("interleaved prose while tool args held", (_events, text) => provider.ready(mode) && text.includes(marker));
    const initialTurnId = records().slice(start).find(event => event.method === "turn/started")?.payload?.params?.turn?.id;
    if (!initialTurnId) throw new Error("Missing initial active turn");
    if (mode === "follow") {
      mux(["send-keys", "-t", session, "-l", `queue-output queued ${mode}`]);
      mux(["send-keys", "-t", session, "M-Enter"]);
      await waitFor("follow-up persisted", events => rpcSucceeded(events.slice(start), "session/queue/push"));
    } else {
      await send(`queue-output queued ${mode}`);
      await waitFor("steer accepted", events => rpcSucceeded(events.slice(start), "turn/steer"));
    }
    provider.release(mode);
    await waitFor("queued response completed", (events, text) => text.includes(`QUEUED_REPLY_${mode.toUpperCase()}`)
      && queuedTurnCompleted(events.slice(start), mode, initialTurnId));
    // Real editor input flushes pending UI events before capturing one frame,
    // not accumulated ANSI writes or scrollback (which may redraw old text).
    const barrier = `queue-render-${mode}`;
    mux(["send-keys", "-t", session, "-l", barrier]);
    await waitFor("keyboard render barrier", () => mux(["capture-pane", "-t", session, "-p"]).stdout.includes(barrier));
    const frame = mux(["capture-pane", "-t", session, "-p"]).stdout;
    if (frame.split(marker).length - 1 !== 1) throw new Error(`Repeated or missing assistant cell: ${marker}\n${frame}`);
    mux(["send-keys", "-t", session, "C-u"]);
    const timeline = records().slice(start).filter(event => ["item/started", "item/completed"].includes(event.method));
    const prose = timeline.filter(event => event.method === "item/completed" && event.payload?.params?.item?.item?.text === marker);
    if (prose.length !== 1) throw new Error("Expected exactly one authoritative prose completion");
    const refresh = timeline.findIndex(event => event.method === "item/started"
      && event.payload?.params?.item?.item?.callId === `tool-${mode}`
      && JSON.stringify(event.payload?.params?.item?.item).includes("tool finished"));
    if (refresh < 0 || timeline.indexOf(prose[0]) >= refresh) throw new Error("Prose not completed before tool-input refresh");
    if (provider.errors.length) throw new Error(provider.errors.join("\n"));
  }
}
