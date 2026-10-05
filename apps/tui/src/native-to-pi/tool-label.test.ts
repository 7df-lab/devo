import { test } from "node:test";
import assert from "node:assert/strict";
import { formatNativeToolLabel } from "./tool-label.js";

test("formats flat MCP tool names as a readable tool and server label", () => {
  assert.equal(formatNativeToolLabel("mcp__echo_test__mcp_echo"), "MCP · Mcp Echo (Echo Test)");
});

test("uses a friendly Python label and preserves malformed names", () => {
  assert.equal(formatNativeToolLabel("ipython"), "Python");
  assert.equal(formatNativeToolLabel("mcp__server__tool__suffix"), "mcp__server__tool__suffix");
});
