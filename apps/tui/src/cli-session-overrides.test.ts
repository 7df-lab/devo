import assert from "node:assert/strict";
import { test } from "node:test";
import { applyCliSessionOverrides, nativeServerArgs } from "./cli-session-overrides.js";

test("CLI full-access flag updates the Native session before interaction", async () => {
  const calls: string[] = [];
  const connection = {
    async updatePermissionProfile(profile: string) {
      calls.push(profile);
      return profile;
    },
  };
  await applyCliSessionOverrides(connection, "1");
  assert.deepEqual(calls, ["fullAccess"]);
  await applyCliSessionOverrides(connection, undefined);
  await applyCliSessionOverrides(connection, "0");
  assert.deepEqual(calls, ["fullAccess"]);
});

test("CLI permission override errors stop startup instead of silently falling back", async () => {
  const connection = {
    async updatePermissionProfile() {
      throw new Error("permission update rejected");
    },
  };
  await assert.rejects(applyCliSessionOverrides(connection, "1"), /permission update rejected/);
});

test("CLI log level reaches only the explicit stdio server launch", () => {
  assert.deepEqual(nativeServerArgs("debug"), ["--log-level", "debug", "server", "--transport", "stdio"]);
  assert.deepEqual(nativeServerArgs(undefined), ["server", "--transport", "stdio"]);
  assert.deepEqual(nativeServerArgs("unexpected"), ["server", "--transport", "stdio"]);
});
