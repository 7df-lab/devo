import { test } from "node:test";
import assert from "node:assert/strict";
import { mapSkillSourceInfo } from "./native-connection-ops.js";

test("system skills map to builtin autocomplete tag", () => {
  const info = mapSkillSourceInfo({
    name: "goal",
    path: "/skills/goal/SKILL.md",
    scope: "system",
    source: "System",
  });
  assert.equal(info.source, "builtin");
  assert.equal(info.scope, "path");
});

test("user skills keep user scope", () => {
  const info = mapSkillSourceInfo({
    name: "mine",
    path: "/home/.agents/skills/mine/SKILL.md",
    scope: "user",
    source: "User",
  });
  assert.equal(info.scope, "user");
  assert.notEqual(info.source, "builtin");
});

test("repo skills map to project scope", () => {
  const info = mapSkillSourceInfo({
    name: "local",
    path: "/repo/.agents/skills/local/SKILL.md",
    scope: "repo",
    source: "Workspace",
  });
  assert.equal(info.scope, "project");
});
