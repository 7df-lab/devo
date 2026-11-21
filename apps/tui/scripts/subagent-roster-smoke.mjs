/** Real Python spawn + server queue + TUI roster; only the HTTP model is controlled. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

export async function startSubagentRosterProvider(home) {
  const held = new Map();
  let spawned = false;
  let childResultRequests = 0;
  const errors = [];
  const server = createServer(async (request, response) => {
    try {
      let raw = "";
      for await (const chunk of request) raw += chunk;
      const body = JSON.parse(raw);
      if (body.stream !== true) {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ id: "title", choices: [{ index: 0, message: { role: "assistant", content: "Roster probe" }, finish_reason: "stop" }] }));
        return;
      }
      const lastUser = JSON.stringify(body.messages?.filter(m => m.role === "user").at(-1)?.content ?? "");
      const child = lastUser.includes("ROSTER_CHILD_TASK");
      const root = lastUser.includes("ROSTER_ROOT_TASK");
      const result = lastUser.includes("[Host notification: ") && lastUser.includes("ROSTER_CHILD_FINISHED");
      if (!child && !root && !result) throw new Error("unexpected model request");
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (delta, finish_reason = null) => response.write(`data: ${JSON.stringify({ id: "roster", choices: [{ index: 0, delta, finish_reason }] })}\n\n`);
      if (result) {
        childResultRequests++;
        send({ role: "assistant", content: "Parent received the work from the subagent." }, "stop");
        response.end("data: [DONE]\n\n");
        return;
      }
      if (root && !spawned) {
        spawned = true;
        send({ role: "assistant", tool_calls: [{ index: 0, id: "spawn-probe", type: "function", function: {
          name: "ipython", arguments: JSON.stringify({ code: "worker = await rlm.spawn('ROSTER_CHILD_TASK', name='roster-worker')\nprint('ROSTER_SPAWN_RETURNED')" }),
        } }] });
        send({}, "tool_calls");
        response.end("data: [DONE]\n\n");
        return;
      }
      const key = child ? "child" : "parent";
      if (held.has(key)) throw new Error(`duplicate ${key} request`);
      send({ role: "assistant" });
      held.set(key, response);
      response.on("close", () => { if (held.get(key) === response) held.delete(key); });
    } catch (error) {
      errors.push(String(error));
      response.destroy(error);
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
    models: { "deepseek-v4-flash": {} },
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "local-fixture-only" } }), { mode: 0o600 });
  return {
    errors,
    get childResultRequests() { return childResultRequests; },
    ready: key => held.has(key),
    release(key) {
      const response = held.get(key);
      if (!response) throw new Error(`missing held ${key}`);
      held.delete(key);
      response.end(`data: ${JSON.stringify({ id: "roster", choices: [{ index: 0, delta: { content: key === "child" ? "ROSTER_CHILD_FINISHED" : "ROSTER_PARENT_FINISHED" }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`);
    },
    close() { server.closeAllConnections(); server.close(); },
  };
}

export async function checkSubagentRoster({ provider, mux, session, send, waitFor, records }) {
  const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
  await send("ROSTER_ROOT_TASK");
  await waitFor("Python spawn returned while child is held", () => provider.ready("parent") && provider.ready("child"));
  mux(["send-keys", "-t", session, "-l", "roster-render-probe"]);
  await waitFor("live child count after render barrier", () => screen().includes("roster-render-probe") && /Subagents\s+1\b/.test(screen()));
  if (/\[Agent spawned\]/i.test(screen())) throw new Error("Routine spawn leaked into the parent chat");
  mux(["send-keys", "-t", session, "C-u"]);
  mux(["send-keys", "-t", session, "Down"]);
  await waitFor("focused tray counts only the running worker", () => /Subagents\s*·\s*1 running\s*│/.test(screen()));
  provider.release("child");
  await waitFor("running child count drops without a new parent turn", () => /Subagents\s*·\s*0 running\s*│/.test(screen()));
  const envelope = event => event.payload?.params?.item;
  const turn = event => event.payload?.params?.turn;
  const fromWorker = origin => origin?.source === "agent" && origin.senderName === "roster-worker"
    && origin.eventId?.startsWith("child:") && origin.eventId.endsWith(":completed");
  let parent;
  await waitFor("child completion queued as steering while parent streams", events => {
    parent = events.find(event => event.method === "item/started"
      && envelope(event)?.item?.type === "userMessage"
      && !envelope(event).item.origin
      && envelope(event).item.content?.some(part => part.text === "ROSTER_ROOT_TASK"));
    return parent && events.some(event => event.method === "queue/updated"
      && event.payload?.params?.sessionId === envelope(parent).sessionId
      && event.payload.params.queue?.some(entry => entry.delivery === "steer" && fromWorker(entry.origin)));
  });
  if (!provider.ready("parent") || provider.childResultRequests !== 0) {
    throw new Error("Child delivery must wait for the parent's model boundary without interrupting it");
  }
  const { sessionId, turnId } = envelope(parent);
  mux(["send-keys", "-t", session, "Escape"]);
  const result = "Steering: roster-worker: ROSTER_CHILD_FINISHED";
  await waitFor("compact child message is visible in the pending queue", () => screen().includes(result));
  if (/\[system\] Child agent/.test(screen())) throw new Error("Raw child completion leaked into the queue preview");
  mux(["resize-window", "-t", session, "-x", "100", "-y", "16"]);
  mux(["send-keys", "-t", session, "-l", "roster-dock-probe"]);
  const dockRows = () => {
    const lines = screen().split("\n");
    return [lines.findIndex(line => line.includes(result)), lines.findIndex(line => line.includes("roster-dock-probe"))];
  };
  await waitFor("queue just above the composer at a short viewport height", () => {
    const [queueRow, editorRow] = dockRows();
    return queueRow >= 0 && editorRow > queueRow && editorRow - queueRow <= 5;
  });
  const pinnedRows = dockRows();
  mux(["send-keys", "-t", session, "PPage"]);
  await waitFor("scrolling the transcript leaves queue and composer pinned", () => {
    const [queueRow, editorRow] = dockRows();
    return screen().includes("to follow") && queueRow === pinnedRows[0] && editorRow === pinnedRows[1];
  });
  mux(["send-keys", "-t", session, "C-End", "C-u"]);
  mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
  provider.release("parent");
  await waitFor("same-turn child result and parent response", events => {
    const admitted = events.findIndex(event => event.method === "item/started"
      && envelope(event)?.turnId === turnId && envelope(event).item?.entry === "steer"
      && fromWorker(envelope(event).item.origin));
    const finished = events.findIndex(event => event.method === "turn/completed"
      && turn(event)?.id === turnId && turn(event).status === "completed");
    return admitted >= 0 && finished > admitted && provider.childResultRequests === 1;
  });
  mux(["send-keys", "-t", session, "-l", "roster-delivery-render-probe"]);
  await waitFor("queue clears when the child message enters the turn", () => {
    const text = screen();
    return text.includes("roster-delivery-render-probe") && /Subagents\s+0 ·/.test(text) && !text.includes(result);
  });
  if (/Delivered:|\[Agent (?:spawned|message)\]/i.test(screen())) throw new Error("Unexpected delivery notice in the parent chat");
  if (records().filter(event => event.method === "turn/started" && turn(event)?.sessionId === sessionId).length !== 1) {
    throw new Error("Child completion started a second parent turn");
  }
  if (provider.errors.length) throw new Error(provider.errors.join("\n"));
  mux(["send-keys", "-t", session, "C-u", "M-a"]);
  await waitFor("focus the Subagents tray", () => /Subagents\s*·\s*0 running\s*│/.test(screen()));
  mux(["send-keys", "-t", session, "Enter"]);
  await waitFor("discover the finished child in the scoped agents list", () => {
    const text = screen();
    return text.includes("› subagents") && text.includes("Idle (1)") && text.includes("roster-worker");
  });
  mux(["send-keys", "-t", session, "Down", "Enter"]);
  await waitFor("finished child transcript is still inspectable", () => {
    const text = screen();
    return text.includes("ROSTER_CHILD_TASK") && text.includes("ROSTER_CHILD_FINISHED");
  });
}
