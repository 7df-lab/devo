import assert from "node:assert/strict";
import test from "node:test";
import {
  BUILTIN_SLASH_COMMANDS,
  parseSlashCommand,
  resolveSlashCommand,
} from "../lib/coding-agent/src/core/slash-commands.js";
import { CombinedAutocompleteProvider } from "../lib/tui/src/autocomplete.js";

for (const [name, alias, args] of [
  ["schedule", "heartbeat", "every 5m Check build progress"],
  ["schedules", "heartbeats", ""],
]) {
  test(`/${name} is canonical and /${alias} still works`, async () => {
    assert.ok(BUILTIN_SLASH_COMMANDS.some((command) => command.name === name));
    assert.ok(!BUILTIN_SLASH_COMMANDS.some((command) => command.name === alias));
    for (const originalName of [name, alias]) {
      const parsed = parseSlashCommand(`/${originalName} ${args}`);
      assert.ok(parsed);
      assert.deepEqual(resolveSlashCommand(parsed), {
        name, args, originalName, isAlias: originalName === alias,
      });
    }
    const autocomplete = new CombinedAutocompleteProvider([...BUILTIN_SLASH_COMMANDS], process.cwd());
    const text = `/${alias}`;
    const suggestions = await autocomplete.getSuggestions([text], 0, text.length, {
      signal: new AbortController().signal,
    });
    assert.ok(suggestions?.items.some((item) => item.value === name));
    assert.ok(!suggestions?.items.some((item) => item.value === alias));
  });
}
