import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { getKeybindings, visibleWidth } from "@earendil-works/pi-tui";
import { ThinkingSelectorComponent } from "../lib/coding-agent/src/modes/interactive/components/thinking-selector.js";

initTheme("dark");

test("effort selector explains purpose and actions without changing current choice", () => {
  const selected: string[] = [];
  let cancelled = 0;
  const selector = new ThinkingSelectorComponent("medium", ["low", "medium", "high"],
    value => selected.push(value), () => cancelled++);
  const text = stripVTControlCharacters(selector.render(80).join("\n"));
  assert.match(text, /Reasoning effort/);
  assert.match(text, /↑\/↓ move.*Enter select.*Esc cancel/);
  selector.getSelectList().handleInput("\r");
  assert.deepEqual(selected, ["medium"]);
  selector.getSelectList().handleInput("\u001b[B");
  selector.getSelectList().handleInput("\r");
  assert.deepEqual(selected, ["medium", "high"]);
  selector.getSelectList().handleInput("\u001b");
  assert.equal(cancelled, 1);
});

test("effort selector wraps help at narrow widths without dropping choices", () => {
  for (const width of [20, 40, 80]) {
    const selector = new ThinkingSelectorComponent("medium", ["low", "medium", "high"], () => {}, () => {});
    const lines = selector.render(width);
    assert.ok(lines.every(line => visibleWidth(line) <= width), `overflow at ${width}`);
    const text = stripVTControlCharacters(lines.join("\n"));
    for (const label of ["Reasoning effort", "low", "medium", "high", "select", "cancel"]) {
      assert.ok(text.includes(label), `missing ${label} at ${width}`);
    }
  }
});

test("effort help follows remapped selection keys", () => {
  const keys = getKeybindings();
  const original = keys.getUserBindings();
  try {
    keys.setUserBindings({ ...original, "tui.select.up": "k", "tui.select.down": "j",
      "tui.select.confirm": "ctrl+y", "tui.select.cancel": "ctrl+q" });
    const selected: string[] = [];
    let cancelled = false;
    const selector = new ThinkingSelectorComponent("low", ["low", "high"], value => selected.push(value), () => { cancelled = true; });
    const text = stripVTControlCharacters(selector.render(80).join("\n"));
    assert.match(text, /K\/J move.*Ctrl\+Y select.*Ctrl\+Q cancel/);
    selector.getSelectList().handleInput("j");
    selector.getSelectList().handleInput("\u0019");
    selector.getSelectList().handleInput("\u0011");
    assert.deepEqual({ selected, cancelled }, { selected: ["high"], cancelled: true });
  } finally {
    keys.setUserBindings(original);
  }
});
