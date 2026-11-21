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
import { stripAnsi } from "../lib/tui/src/utils.js";

function createViewport(t: TestContext, bindings: KeybindingsConfig = {}) {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager(TUI_KEYBINDINGS, bindings));
  let send = (_data: string) => {};
  let resize = () => {};
  let output = "";
  let draft = "";
  const lines = Array.from({ length: 50 }, (_, index) => `Transcript ${index + 1}`);
  let columns = 80;
  let rows = 12;
  const terminal: Terminal = {
    get columns() { return columns; },
    get rows() { return rows; },
    kittyProtocolActive: false,
    altScreenActive: true,
    mouseTrackingActive: true,
    start(onInput, onResize) { send = onInput; resize = onResize; },
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
  const transcript = { render: () => lines, invalidate() {} };
  ui.addChild(transcript);
  ui.addChild(editor);
  ui.start();
  ui.enterFullscreen({ scroll: [transcript], dock: editor });
  ui.setFocus(editor);
  t.after(() => {
    ui.stop({ flushFullscreen: false });
    setKeybindings(previous);
  });
  return {
    ui,
    lines,
    resize(width: number, height: number) {
      columns = width;
      rows = height;
      resize();
    },
    clearOutput() { output = ""; },
    async settle() {
      await new Promise((resolve) => setTimeout(resolve, 50));
      return output;
    },
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

test("the paused follow hint occupies its own row instead of painting over transcript text", async (t) => {
  const viewport = createViewport(t);
  const fullWidthLine = "const value = ".padEnd(80, "X");
  viewport.lines.fill(fullWidthLine);
  await viewport.render();
  viewport.send("\x1b[5~");

  const paused = stripAnsi((await viewport.render()).split(/\x1b\[\d+;1H\x1b\[2K/).slice(1).join("\n"));
  assert.deepEqual(paused.split("\n").map(line => line.trim()), [
    ...Array(10).fill(fullWidthLine),
    "ctrl+end to follow",
    ">",
  ]);

  viewport.resize(80, 4);
  const cramped = await viewport.render();
  assert.doesNotMatch(cramped, /to follow/);
  assert.equal(cramped.split(fullWidthLine).length - 1, 3);
  assert.equal(viewport.ui.getScrollInfo()?.following, false);
});

test("fullscreen repaints omit shell integration marks without removing styles or links", async (t) => {
  const viewport = createViewport(t);
  const markStart = "\x1b]133;A\x07";
  const markEnd = "\x1b]133;B\x07\x1b]133;C\x1b\\";
  const link = "\x1b]8;;https://example.com\x1b\\\x1b[1mLink\x1b[22m\x1b]8;;\x1b\\";
  for (const [index, line] of viewport.lines.entries()) {
    viewport.lines[index] = `${markStart}${line} ${link}${markEnd}`;
  }

  for (const repaint of [() => {}, () => viewport.send("\x1b[5~"), () => viewport.resize(64, 10)]) {
    repaint();
    const output = await viewport.render();
    assert.doesNotMatch(output, /\x1b\]133;/);
    assert.ok(output.includes(link));
  }

  viewport.ui.exitFullscreen({ flush: false });
  const inline = await viewport.render();
  assert.ok(inline.includes(markStart));
  assert.ok(inline.includes(markEnd));
  assert.ok(inline.includes(link));
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

// A terminal can discard rows while resizing even when its final dimensions
// match the last painted frame. No model event should be needed to restore it.
test("resize round trip repaints unchanged working status without a new turn", async (t) => {
  const viewport = createViewport(t);
  viewport.lines.push("Thinking · 12s");
  await viewport.render();
  viewport.clearOutput();
  viewport.resize(40, 5);
  viewport.resize(80, 12);
  assert.match(await viewport.settle(), /Thinking · 12s/);
  assert.equal(viewport.ui.getScrollInfo()?.following, true);
});

test("resize repaint preserves paused transcript and does not resurrect ended status", async (t) => {
  const viewport = createViewport(t);
  viewport.lines.push("Working · 12s");
  await viewport.render();
  viewport.send("\x1b[5~");
  await viewport.render();
  const before = viewport.ui.getScrollInfo();
  viewport.clearOutput();
  viewport.resize(40, 5);
  viewport.resize(80, 12);
  const paused = await viewport.settle();
  assert.match(paused, /Transcript/);
  assert.deepEqual(viewport.ui.getScrollInfo(), before);
  viewport.lines.pop();
  viewport.send("\x1b[1;5F");
  await viewport.render();
  viewport.clearOutput();
  viewport.resize(40, 5);
  viewport.resize(80, 12);
  assert.doesNotMatch(await viewport.settle(), /Working/);
});
