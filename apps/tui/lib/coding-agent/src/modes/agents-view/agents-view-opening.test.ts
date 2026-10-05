import { test } from "node:test";
import assert from "node:assert/strict";
import { visibleWidth } from "@earendil-works/pi-tui";
import { renderAgentsViewOpeningSession } from "./agents-view-mode.js";

const ansiEscape = /\u001b\[[0-?]*[ -/]*[@-~]/g;

test("opening session screen identifies the target and states that it is read-only", () => {
  const lines = renderAgentsViewOpeningSession(
    { title: "Target session", cwd: "/tmp/workspace", model: "gpt-5.6-luna" },
    80,
    24,
  );
  const screen = lines.join("\n").replace(ansiEscape, "");

  assert.match(screen, /Opening session/);
  assert.match(screen, /Target session/);
  assert.match(screen, /Model: gpt-5\.6-luna/);
  assert.match(screen, /Directory: workspace/);
  assert.match(screen, /read-only until the session is ready/);
});

test("opening session screen clips content to the available width", () => {
  const lines = renderAgentsViewOpeningSession(
    { title: "A very long target session name that cannot fit", cwd: "/tmp/workspace", model: "model-id" },
    24,
    12,
  );
  const screen = lines.join("\n");

  assert.ok(lines.every((line) => visibleWidth(line) <= 24));
  assert.match(screen, /Opening session/);
});
