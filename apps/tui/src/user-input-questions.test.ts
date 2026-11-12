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
