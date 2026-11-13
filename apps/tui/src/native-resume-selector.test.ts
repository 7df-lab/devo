import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";

const session = (id: string, cwd = "/current") => ({ id, path: id, cwd });
function fixture(current: ReturnType<typeof session>[], all = current) {
  const mode = Object.create(InteractiveMode.prototype) as any;
  const scopes: string[] = [], resumed: string[] = [], errors: string[] = [];
  let agents = 0;
  mode.getCurrentCwd = () => "/current";
  mode.agentConnection = { listSavedSessions: async (scope: string) => {
    scopes.push(scope); return scope === "current" ? current : all;
  } };
  mode.handleResumeSession = async (id: string) => { resumed.push(id); };
  mode.showError = (message: string) => errors.push(message);
  mode.requestAgentsView = async () => { agents++; };
  return { mode, scopes, resumed, errors, get agents() { return agents; } };
}
test("resume routes native exact ID through connection catalog, not local JSONL", async () => {
  const f = fixture([session("ses_1234567890abcdef")]);
  await f.mode.handleResumeCommand("  ses_1234567890abcdef  ");
  assert.deepEqual(f.resumed, ["ses_1234567890abcdef"]);
  assert.deepEqual(f.scopes, ["current"]);
  assert.deepEqual(f.errors, []);
});
test("resume prefers global exact ID over local partial ID and supports partial IDs", async () => {
  const f = fixture([session("ses_12345678abcd")], [session("ses_12345678", "/other")]);
  await f.mode.handleResumeCommand("ses_12345678");
  assert.deepEqual(f.resumed, ["ses_12345678"]);
  const g = fixture([session("ses_abcdef0123456789")]);
  await g.mode.handleResumeCommand("ses_abcdef01");
  assert.deepEqual(g.resumed, ["ses_abcdef0123456789"]);
});
test("resume rejects ambiguous and missing IDs without changing session", async () => {
  const f = fixture([session("ses_abcdef011111"), session("ses_abcdef012222")]);
  await f.mode.handleResumeCommand("ses_abcdef01");
  assert.deepEqual(f.resumed, []);
  assert.match(f.errors[0], /Ambiguous saved session/);
  const g = fixture([]);
  await g.mode.handleResumeCommand("nonexistent");
  assert.deepEqual(g.resumed, []);
  assert.match(g.errors[0], /No session found/);
});
test("resume with no selector still opens agents and legacy paths remain supported", async () => {
  const f = fixture([]);
  await f.mode.handleResumeCommand(" ");
  assert.equal(f.agents, 1);
  assert.deepEqual(f.scopes, []);
  await f.mode.handleResumeCommand("/saved/session.jsonl");
  assert.deepEqual(f.resumed, ["/saved/session.jsonl"]);
});
