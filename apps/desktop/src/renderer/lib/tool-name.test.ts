import { describe, expect, test } from "bun:test";
import { formatNativeToolTitle } from "./tool-name";

describe("formatNativeToolTitle", () => {
  test("shows the MCP tool and server without the flattened prefix", () => {
    expect(formatNativeToolTitle("mcp__echo_test__mcp_echo")).toBe("MCP · Mcp Echo (Echo Test)");
  });

  test("uses friendly labels for Python and result rows", () => {
    expect(formatNativeToolTitle("ipython")).toBe("Python");
    expect(formatNativeToolTitle("toolResult")).toBe("Tool result");
  });

  test("preserves malformed tool names", () => {
    expect(formatNativeToolTitle("mcp__server__tool__suffix")).toBe("mcp__server__tool__suffix");
  });
});
