import assert from "node:assert/strict";
import { test } from "node:test";
import { resolveUserInputQuestions } from "../lib/coding-agent/src/modes/interactive/user-input-questions.js";

const questions = [{ id: "path", question: "Which path?", options: [
  { label: "A", description: "First path" }, { label: "B", description: "Second path" },
] }, { id: "name", question: "Your name?" }];

test("Native object options render as strings and all questions return their original IDs", async () => {
  const calls: unknown[] = [];
  const response = await resolveUserInputQuestions(questions, {
    async select(title, options) { calls.push([title, options]); return options[1]; },
    async input(title) { calls.push([title]); return "Ada"; },
  });
  assert.deepEqual(calls, [["Which path?", ["1. A — First path", "2. B — Second path"]], ["Your name?"]]);
  assert.deepEqual(JSON.parse(JSON.stringify(response)), { answers: { path: { answers: ["B"] }, name: { answers: ["Ada"] } } });
});

test("Other text and duplicate labels are unambiguous", async () => {
  const response = await resolveUserInputQuestions([{ ...questions[0], isOther: true }], {
    async select(_, options) { return options.at(-1); }, async input() { return "custom"; },
  });
  assert.deepEqual(JSON.parse(JSON.stringify(response)), { answers: { path: { answers: ["custom"] } } });
});

test("cancel on later question does not send a partial answer", async () => {
  assert.deepEqual(await resolveUserInputQuestions(questions, {
    async select(_, options) { return options[0]; }, async input() { return undefined; },
  }), { cancelled: true });
});

test("abort between questions never opens the next dialog", async () => {
  const controller = new AbortController();
  assert.deepEqual(await resolveUserInputQuestions(questions, {
    async select(_, options) { controller.abort(); return options[0]; },
    async input() { assert.fail("must not open"); },
  }, controller.signal), { cancelled: true });
});

test("malformed question sets fail explicitly", async () => {
  const dialogs = { async select() { return undefined; }, async input() { return "answer"; } };
  await assert.rejects(resolveUserInputQuestions([], dialogs), /no questions/);
  await assert.rejects(resolveUserInputQuestions([{ id: "x" }], dialogs), /Invalid/);
  await assert.rejects(resolveUserInputQuestions([{ id: "x", question: "X" }, { id: "x", question: "Y" }], dialogs), /Duplicate/);
});


test("secret questions never open an unmasked dialog", async () => {
  await assert.rejects(resolveUserInputQuestions([{ ...questions[0], isOther: true, isSecret: true }], {
    async select() { assert.fail("must not open"); }, async input() { assert.fail("must not open"); },
  }), /Secret user input/);
});

test("InteractiveMode opens Native questions and forwards the complete response", async () => {
  const { InteractiveMode } = await import("../lib/coding-agent/src/modes/interactive/interactive-mode.js");
  const mode = Object.create(InteractiveMode.prototype) as any;
  mode.activeConnectionExtensionUiRequests = new Map();
  const replies: unknown[] = [];
  mode.agentConnection = { async respondToExtensionUiRequest(id: string, response: unknown) { replies.push({ id, response }); } };
  mode.showExtensionSelector = async (_title: string, options: string[]) => options[0];
  mode.showExtensionInput = async () => "Ada";
  mode.showError = (error: string) => assert.fail(error);
  await mode.handleConnectionExtensionUiRequest({ id: "12", method: "user_input", userInputRequestId: "q_12", payload: { questions } });
  assert.deepEqual(JSON.parse(JSON.stringify(replies)), [{ id: "12", response: { answers: { path: { answers: ["A"] }, name: { answers: ["Ada"] } } } }]);
  assert.equal(mode.activeConnectionExtensionUiRequests.size, 0);
});

test("InteractiveMode dismisses a question when another controller resolves it", async () => {
  const { InteractiveMode } = await import("../lib/coding-agent/src/modes/interactive/interactive-mode.js");
  const mode = Object.create(InteractiveMode.prototype) as any;
  mode.activeConnectionExtensionUiRequests = new Map();
  const replies: unknown[] = [];
  mode.agentConnection = { async respondToExtensionUiRequest(id: string, response: unknown) { replies.push({ id, response }); } };
  let started!: () => void;
  const ready = new Promise<void>((resolve) => { started = resolve; });
  mode.showExtensionSelector = async (_title: string, _options: string[], options: { signal: AbortSignal }) => new Promise((resolve) => {
    options.signal.addEventListener("abort", () => resolve(undefined), { once: true }); started();
  });
  mode.showExtensionInput = async () => assert.fail("must not open second question");
  mode.showError = (error: string) => assert.fail(error);
  const pending = mode.handleConnectionExtensionUiRequest({ id: "12", method: "user_input", userInputRequestId: "q_12", payload: { questions } });
  await ready;
  mode.cancelConnectionExtensionUiRequests({ userInputRequestId: "q_12" });
  await pending;
  assert.deepEqual(replies, [{ id: "12", response: { cancelled: true } }]);
  assert.equal(mode.activeConnectionExtensionUiRequests.size, 0);
});
