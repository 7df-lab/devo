/** Real Python spawn + server queue + TUI roster; only the HTTP model is controlled. */
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

export async function startSubagentRosterProvider(home) {
  const held = new Map();
  let spawned = false;
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
      if (!child && !root) throw new Error("unexpected model request");
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (delta, finish_reason = null) => response.write(`data: ${JSON.stringify({ id: "roster", choices: [{ index: 0, delta, finish_reason }] })}\n\n`);
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
      response.destroy(error);
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "providers.json"), JSON.stringify({ model: "deepseek/deepseek-v4-flash", provider: { deepseek: {
    credential: "deepseek", enabled: true, base_url: `http://127.0.0.1:${server.address().port}`, wire_api: "openai_chat_completions",
  } } }));
  writeFileSync(join(home, "auth.json"), JSON.stringify({ deepseek: { type: "api_key", key: "local-fixture-only" } }), { mode: 0o600 });
  return {
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

export async function checkSubagentRoster({ provider, mux, session, send, waitFor }) {
  const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
  await send("ROSTER_ROOT_TASK");
  await waitFor("Python spawn returned while child is held", () => provider.ready("parent") && provider.ready("child"));
  mux(["send-keys", "-t", session, "-l", "roster-render-probe"]);
  await waitFor("live child count after render barrier", () => screen().includes("roster-render-probe") && /Subagents\s+1\b/.test(screen()));
  mux(["send-keys", "-t", session, "C-u"]);
  mux(["send-keys", "-t", session, "Down"]);
  await waitFor("focused subagent is running, not idle", () => /Subagents\s*·\s*1 running\s*·\s*0 idle/.test(screen()));
  provider.release("child");
  await waitFor("completed child count updates without new parent turn", () => /Subagents\s*·\s*0 running\s*·\s*1 idle/.test(screen()));
  if (!provider.ready("parent")) throw new Error("parent request must remain held during roster transitions");
  mux(["send-keys", "-t", session, "Escape"]);
  provider.release("parent");
}
