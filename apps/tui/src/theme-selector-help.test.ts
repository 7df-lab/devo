import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { initTheme, setTheme, theme } from "@earendil-works/pi-coding-agent/theme";
import { getKeybindings, visibleWidth } from "@earendil-works/pi-tui";
import { ThemeSelectorComponent } from "../lib/coding-agent/src/modes/interactive/components/theme-selector.js";

initTheme("dark");

test("theme picker distinguishes preview, save and restore without changing callbacks", () => {
  const selected: string[] = [];
  const previewed: string[] = [];
  let cancelled = 0;
  const selector = new ThemeSelectorComponent("dark", value => selected.push(value),
    () => cancelled++, value => previewed.push(value));
  const text = stripVTControlCharacters(selector.render(80).join("\n"));
  assert.match(text, /Theme preview/);
  assert.match(text, /↑\/↓ preview.*Enter save.*Esc restore/);
  selector.getSelectList().handleInput("\u001b[B");
  assert.deepEqual({ selected, previewed, cancelled }, { selected: [], previewed: ["light"], cancelled: 0 });
  selector.getSelectList().handleInput("\u001b");
  assert.deepEqual({ selected, previewed, cancelled }, { selected: [], previewed: ["light"], cancelled: 1 });
  selector.getSelectList().handleInput("\r");
  assert.deepEqual(selected, ["light"]);
});

test("theme picker wraps preview help at narrow widths", () => {
  for (const width of [20, 40, 80]) {
    const selector = new ThemeSelectorComponent("light", () => {}, () => {}, () => {});
    const lines = selector.render(width);
    assert.ok(lines.every(line => visibleWidth(line) <= width), `overflow at ${width}`);
    const text = stripVTControlCharacters(lines.join("\n"));
    for (const label of ["Theme preview", "dark", "light", "save", "restore"]) assert.ok(text.includes(label));
  }
});

test("theme picker help uses actual configured keys", () => {
  const keys = getKeybindings();
  const original = keys.getUserBindings();
  try {
    keys.setUserBindings({ ...original, "tui.select.up": "k", "tui.select.down": "j",
      "tui.select.confirm": "ctrl+y", "tui.select.cancel": "ctrl+q" });
    const selected: string[] = [];
    const previewed: string[] = [];
    let cancelled = false;
    const selector = new ThemeSelectorComponent("dark", value => selected.push(value),
      () => { cancelled = true; }, value => previewed.push(value));
    const text = stripVTControlCharacters(selector.render(80).join("\n"));
    assert.match(text, /K\/J preview.*Ctrl\+Y save.*Ctrl\+Q restore/);
    selector.getSelectList().handleInput("j");
    selector.getSelectList().handleInput("\u0019");
    selector.getSelectList().handleInput("\u0011");
    assert.deepEqual({ selected, previewed, cancelled }, { selected: ["light"], previewed: ["light"], cancelled: true });
  } finally {
    keys.setUserBindings(original);
  }
});


test("theme help tracks live preview colors on the same selector", () => {
  const selector = new ThemeSelectorComponent("dark", () => {}, () => {}, () => {});
  try {
    for (const name of ["dark", "light", "dark"]) {
      assert.equal(setTheme(name, true).success, true);
      selector.invalidate();
      const rendered = selector.render(80).join("\n");
      assert.ok(rendered.includes(theme.fg("accent", "Theme preview")), name);
      assert.ok(rendered.includes(theme.fg("muted", " preview  ")), name);
    }
  } finally {
    setTheme("dark", true);
  }
});
