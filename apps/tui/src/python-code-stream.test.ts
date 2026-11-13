import assert from "node:assert/strict";
import { test } from "node:test";
import { createNativeToPiToolState, projectToolItemStarted, projectToolCallInputDelta } from "./native-to-pi/tools.js";
import { IPythonCellComponent, getIpythonCodeFromArgs } from "../lib/coding-agent/src/modes/interactive/components/ipython-cell.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

test("Native Python JSON fragments render source incrementally without executing or duplicating", () => {
  initTheme();
  const tools = createNativeToPiToolState();
  projectToolItemStarted({ id: "item" }, { type: "toolCall", callId: "call", toolName: "ipython", input: {} }, tools);
  const cell = new IPythonCellComponent({ code: "", isPartial: true, argsComplete: false });
  // Use valid JSON source chunks, split within a string escape too.
  const json = JSON.stringify({ code: 'first = 1\nsecond = "quoted"\nthird = 3' });
  const cut = json.indexOf('\\"');
  const pieces = [json.slice(0, cut + 1), json.slice(cut + 1, -2), json.slice(-2)];
  let latest = "";
  for (const piece of pieces) {
    const events = projectToolCallInputDelta({ itemId: "item", delta: JSON.stringify({ tool_use_id: "call", partial_json: piece }) }, tools);
    assert.ok(events.every(event => event.type === "tool_args_update"));
    latest = getIpythonCodeFromArgs(events.at(-1)?.args);
    cell.update({ code: latest, isPartial: true, argsComplete: false });
    assert.match(cell.render(100).join("\n"), /first = 1/);
  }
  assert.equal(latest, 'first = 1\nsecond = "quoted"\nthird = 3');
  assert.equal(cell.render(100).join("\n").match(/third = 3/g)?.length, 1);
});
