import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { PermissionSelectorComponent } from "../lib/coding-agent/src/modes/interactive/components/permission-selector.js";

initTheme("dark");

test("auto review explains pre-execution review and possible user approval", () => {
  const selected: string[] = [];
  let cancelled = 0;
  const selector = new PermissionSelectorComponent("autoReview", value => selected.push(value), () => cancelled++);
  const text = stripVTControlCharacters(selector.render(100).join("\n"));
  assert.match(text, /Review before approval; may ask you/);
  assert.doesNotMatch(text, /review afterwards|Auto-approve tools/);
  selector.getSelectList().handleInput("\r");
  selector.getSelectList().handleInput("\u001b");
  assert.deepEqual({selected, cancelled}, {selected: ["autoReview"], cancelled: 1});
});

test("permission copy does not change navigation or cancellation at narrow widths", () => {
  for (const width of [20, 40, 100]) {
    const selected: string[] = [];
    let cancelled = false;
    const selector = new PermissionSelectorComponent("default", value => selected.push(value), () => { cancelled = true; });
    assert.match(stripVTControlCharacters(selector.render(width).join("\n")), /default/);
    selector.getSelectList().handleInput("\u001b[B");
    selector.getSelectList().handleInput("\r");
    selector.getSelectList().handleInput("\u001b");
    assert.deepEqual({selected, cancelled}, {selected: ["autoReview"], cancelled: true});
  }
});
