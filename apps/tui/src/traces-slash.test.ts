/**
 * Verifies `/traces` slash parsing for the Native TUI.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { parseTracesSlash } from "./native-connection-ops.js";
import { DEFAULT_PREVIEW_LIMIT, MAX_PREVIEW_LIMIT } from "./native-traffic-log.js";

test("parseTracesSlash covers status, on/off, and preview forms", () => {
  assert.deepEqual(parseTracesSlash("/traces"), { kind: "status" });
  assert.deepEqual(parseTracesSlash("/traces status"), { kind: "status" });
  assert.deepEqual(parseTracesSlash("/traces on"), { kind: "enable" });
  assert.deepEqual(parseTracesSlash("/traces enable"), { kind: "enable" });
  assert.deepEqual(parseTracesSlash("/traces off"), { kind: "disable" });
  assert.deepEqual(parseTracesSlash("/traces disable"), { kind: "disable" });
  assert.deepEqual(parseTracesSlash("/traces preview"), {
    kind: "preview",
    limit: DEFAULT_PREVIEW_LIMIT,
  });
  assert.deepEqual(parseTracesSlash("/traces preview 5"), { kind: "preview", limit: 5 });
  assert.deepEqual(parseTracesSlash("/traces preview 9999"), {
    kind: "preview",
    limit: MAX_PREVIEW_LIMIT,
  });
});

test("parseTracesSlash rejects unknown subcommands and non-commands", () => {
  assert.equal(parseTracesSlash("/traces upload"), null);
  assert.equal(parseTracesSlash("/traces preview 0"), null);
  assert.equal(parseTracesSlash("/traces preview abc"), null);
  assert.equal(parseTracesSlash("/tracesx"), null);
  assert.equal(parseTracesSlash("show traces"), null);
});
