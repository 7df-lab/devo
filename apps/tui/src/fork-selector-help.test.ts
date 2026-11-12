import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { getKeybindings, visibleWidth } from "@earendil-works/pi-tui";
import { UserMessageSelectorComponent } from "../lib/coding-agent/src/modes/interactive/components/user-message-selector.js";
initTheme("dark");
const messages = Array.from({ length: 50 }, (_, i) => ({ id: String(i), text: `message-${i} 中文 long message\nsecond line` }));
const plain = (selector: UserMessageSelectorComponent) => stripVTControlCharacters(selector.render(80).join("\n"));
test("fork fits short terminals with selected point and help", () => {
  for (const width of [20, 40, 100]) {
    const selector = new UserMessageSelectorComponent(messages, () => {}, () => {}, undefined, () => 24);
    const lines = selector.render(width);
    assert.ok(lines.length <= 20, `height ${lines.length} at width ${width}`);
    assert.ok(lines.every(line => visibleWidth(line) <= width), `overflow at ${width}`);
    const text = stripVTControlCharacters(lines.join("\n"));
    for (const label of ["message-49", "fork", "cancel", "50/50"]) assert.ok(text.includes(label), label);
  }
});
test("resize preserves choice and every point remains reachable", () => {
  let rows = 40;
  const selected: string[] = [];
  let cancelled = 0;
  const selector = new UserMessageSelectorComponent(messages, id => selected.push(id), () => cancelled++, "25", () => rows);
  const tall = selector.render(100).length;
  rows = 24;
  assert.ok(selector.render(40).length < tall);
  selector.getMessageList().handleInput("\r");
  assert.deepEqual(selected, ["25"]);
  for (let i = 0; i < 50; i++) {
    selector.getMessageList().handleInput("\u001b[B");
    selector.render(40);
    selector.getMessageList().handleInput("\r");
  }
  assert.equal(new Set(selected).size, 50);
  selector.getMessageList().handleInput("\u001b");
  assert.equal(cancelled, 1);
  assert.equal(selected.length, 51);
});
test("fork hints and actions follow remapped keys", () => {
  const keys = getKeybindings();
  const original = keys.getUserBindings();
  try {
    keys.setUserBindings({ ...original, "tui.select.up": "k", "tui.select.down": "j", "tui.select.confirm": "ctrl+y", "tui.select.cancel": "ctrl+q" });
    const selected: string[] = [];
    let cancelled = false;
    const selector = new UserMessageSelectorComponent(messages, id => selected.push(id), () => { cancelled = true; });
    assert.match(plain(selector), /K\/J move.*Ctrl\+Y fork.*Ctrl\+Q cancel/);
    selector.getMessageList().handleInput("k");
    selector.getMessageList().handleInput("\u0019");
    selector.getMessageList().handleInput("\u0011");
    assert.deepEqual({ selected, cancelled }, { selected: ["48"], cancelled: true });
  } finally { keys.setUserBindings(original); }
});
test("empty fork history cannot select and can cancel", () => {
  let cancelled = false;
  const selector = new UserMessageSelectorComponent([], () => assert.fail("empty selection"), () => { cancelled = true; });
  assert.match(plain(selector), /No user messages found/);
  selector.getMessageList().handleInput("\u001b[A");
  selector.getMessageList().handleInput("\r");
  selector.getMessageList().handleInput("\u001b");
  assert.equal(cancelled, true);
});
