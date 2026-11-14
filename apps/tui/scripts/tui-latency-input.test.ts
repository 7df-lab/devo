import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { CombinedAutocompleteProvider } from "../lib/tui/src/autocomplete.js";
import { Editor } from "../lib/tui/src/components/editor.js";
import type { TUI } from "../lib/tui/src/tui.js";
import { BUILTIN_SLASH_COMMANDS } from "../lib/coding-agent/src/core/slash-commands.js";
import { NEW_SESSION_INPUT } from "./tui-latency-support.mjs";

// Exercise real editor/completion input, not a terminal-rendering substitute.
// PTY rendering remains independently verified by the benchmark in CI.
test("benchmark /new input submits once instead of only accepting argument completion", async () => {
  const identity = (text: string) => text;
  const tui = { requestRender() {}, showOverlay() { return { hide() {} }; } } as unknown as TUI;
  const editor = new Editor(tui, { borderColor: identity, selectList: {
    selectedPrefix: identity, selectedText: identity, description: identity, scrollInfo: identity, noMatch: identity,
  } });
  editor.setAutocompleteProvider(new CombinedAutocompleteProvider([...BUILTIN_SLASH_COMMANDS], process.cwd()));
  const submitted: string[] = [];
  editor.onSubmit = text => { submitted.push(text); };
  editor.handleInput("/new");
  await setImmediate();
  assert.equal(editor.isShowingAutocomplete(), true);
  editor.handleInput("\r");
  assert.deepEqual(submitted, [], "first Enter on bare /new only completes the argument separator");
  assert.equal(editor.getText(), "/new ");

  editor.setText("");
  editor.handleInput(NEW_SESSION_INPUT);
  await setImmediate();
  assert.equal(editor.isShowingAutocomplete(), false);
  editor.handleInput("\r");
  assert.deepEqual(submitted.map(text => text.trim()), ["/new"]);
});
