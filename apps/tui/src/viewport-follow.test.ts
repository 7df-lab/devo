import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  getKeybindings,
  KeybindingsManager,
  setKeybindings,
  TUI_KEYBINDINGS,
  type KeybindingsConfig,
} from "../lib/tui/src/keybindings.js";
import type { Terminal } from "../lib/tui/src/terminal.js";
import { TUI } from "../lib/tui/src/tui.js";

function createViewport(t: TestContext, bindings: KeybindingsConfig = {}) {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager(TUI_KEYBINDINGS, bindings));
  let send = (_data: string) => {};
  let output = "";
  let draft = "";
  const lines = Array.from({ length: 50 }, (_, index) => `Transcript ${index + 1}`);
  const terminal: Terminal = {
    columns: 80,
    rows: 12,
    kittyProtocolActive: false,
    altScreenActive: true,
    mouseTrackingActive: true,
    start(onInput) { send = onInput; },
    stop() {},
    async drainInput() {},
    write(data) { output += data; },
    moveBy() {},
    hideCursor() {},
    showCursor() {},
    clearLine() {},
    clearFromCursor() {},
    clearScreen() {},
    enterAltScreen() {},
    leaveAltScreen() {},
    setMouseTracking() {},
    setTitle() {},
    setProgress() {},
  };
  const editor = {
    render: () => [`> ${draft}`],
    invalidate() {},
    handleInput(data: string) { draft += data; },
  };
  const ui = new TUI(terminal);
  ui.start();
  ui.enterFullscreen({ scroll: [{ render: () => lines, invalidate() {} }], dock: editor });
  ui.setFocus(editor);
  t.after(() => {
    ui.stop({ flushFullscreen: false });
    setKeybindings(previous);
  });
  return {
    ui,
    lines,
    send(data: string) { send(data); },
    draft: () => draft,
    async render() {
      output = "";
      ui.requestRender(true);
      await new Promise<void>((resolve) => process.nextTick(resolve));
      return output;
    },
  };
}

test("follow defaults to Ctrl+End and retains Ctrl+Shift+Down", () => {
  const bindings = new KeybindingsManager(TUI_KEYBINDINGS);
  assert.deepEqual(bindings.getKeys("tui.viewport.follow"), ["ctrl+end", "ctrl+shift+down"]);
  for (const sequence of ["\x1b[1;5F", "\x1b[8;5~", "\x1b[1;5:1F", "\x1b[57424;5u", "\x1b[1;6B"]) {
    assert.equal(bindings.matches(sequence, "tui.viewport.follow"), true, JSON.stringify(sequence));
  }
});

test("Ctrl+End resumes following after keyboard or mouse scrolling without changing the draft", async (t) => {
  const viewport = createViewport(t);
  await viewport.render();
  viewport.send("unfinished draft");
  for (const scroll of ["\x1b[5~", "\x1b[<64;10;5M"]) {
    viewport.send(scroll);
    assert.equal(viewport.ui.getScrollInfo()?.following, false);
    assert.match(await viewport.render(), /ctrl\+end to follow/);
    viewport.lines.push("New output while paused");
    await viewport.render();
    viewport.send("\x1b[1;5F");
    assert.deepEqual(viewport.ui.getScrollInfo(), {
      following: true,
      linesAbove: viewport.lines.length - 11,
      linesBelow: 0,
    });
    assert.doesNotMatch(await viewport.render(), /to follow/);
    viewport.lines.push("New output after following resumes");
    await viewport.render();
    assert.deepEqual(viewport.ui.getScrollInfo(), {
      following: true,
      linesAbove: viewport.lines.length - 11,
      linesBelow: 0,
    });
    assert.equal(viewport.draft(), "unfinished draft");
  }
});

test("the legacy follow shortcut still resumes following when delivered by the terminal", async (t) => {
  const viewport = createViewport(t);
  await viewport.render();
  const bottom = viewport.ui.getScrollInfo();
  viewport.send("\x1b[5~");
  viewport.send("\x1b[1;6B");
  assert.deepEqual(viewport.ui.getScrollInfo(), bottom);
});

test("follow hints and input honor a custom binding", async (t) => {
  const viewport = createViewport(t, { "tui.viewport.follow": "f6" });
  await viewport.render();
  const bottom = viewport.ui.getScrollInfo();
  viewport.send("\x1b[5~");
  assert.match(await viewport.render(), /f6 to follow/);
  viewport.send("\x1b[17~");
  assert.deepEqual(viewport.ui.getScrollInfo(), bottom);
});

test("disabling the follow binding does not advertise an inactive shortcut", async (t) => {
  const viewport = createViewport(t, { "tui.viewport.follow": [] });
  await viewport.render();
  viewport.send("\x1b[5~");
  assert.equal(viewport.ui.getScrollInfo()?.following, false);
  assert.doesNotMatch(await viewport.render(), /to follow/);
});
