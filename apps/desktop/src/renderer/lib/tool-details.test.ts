import { describe, expect, test } from "bun:test";
import { formatNativeToolDetails, getNativePythonToolCode } from "./tool-details";

describe("formatNativeToolDetails", () => {
  test("identifies Python tool calls and returns their source", () => {
    const code = "goal_state = await goal.get()\nprint(goal_state)";
    const item = { type: "toolCall", toolName: "ipython", input: { code } };
    expect(getNativePythonToolCode(item)).toBe(code);
    expect(getNativePythonToolCode({ type: "toolResult", output: code })).toBeUndefined();
    expect(formatNativeToolDetails(item)).toBe(code);
  });

  test("shows textual tool output instead of nested JSON", () => {
    const output = "PYTHON_SKILL_OK goal.get\nMCP-ECHO-OK desktop-devo-qa\n";
    expect(
      formatNativeToolDetails({
        type: "toolResult",
        output: { content: [{ type: "text", text: output }], details: { stdout: output, stderr: "" } },
      }),
    ).toBe(output);
  });

  test("keeps structured input readable when no text projection exists", () => {
    expect(formatNativeToolDetails({ type: "toolCall", input: { text: "hello" } })).toBe(
      '{\n  "text": "hello"\n}',
    );
  });
});
