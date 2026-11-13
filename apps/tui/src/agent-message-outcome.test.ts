import { test } from "node:test";
import assert from "node:assert/strict";
import { assertAgentMessageDisplay } from "../scripts/agent-message-outcome.mjs";

test("agent display TTY oracle requires body and rejects original system preamble", () => {
  assert.doesNotThrow(() => assertAgentMessageDisplay("Agent message\nFrom: worker · completed\nCHILD_RESULT", "CHILD_RESULT"));
  for (const pane of ["", "Agent message", "CHILD_RESULT", "Agent message\n[system] Child agent /worker (ses_child) completed.\nCHILD_RESULT"]) {
    assert.throws(() => assertAgentMessageDisplay(pane, "CHILD_RESULT"));
  }
});
