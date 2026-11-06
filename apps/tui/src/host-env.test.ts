import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

for (const explicitOverrides of [false, true]) {
  test(`agent directory setup is idempotent with ${explicitOverrides ? "explicit" : "default"} environment`, () => {
    const directory = mkdtempSync(join(tmpdir(), "devo-host-env-"));
    try {
      const home = join(directory, "home");
      const env = { ...process.env };
      delete env.DEVO_CODING_AGENT_DIR;
      delete env.DEVO_HOME;
      delete env.PI_SKIP_VERSION_CHECK;
      const expected = explicitOverrides
        ? {
            agentDir: join(directory, "agent-override"),
            home: join(directory, "home-override"),
            skipVersionCheck: "0",
          }
        : { agentDir: home, home, skipVersionCheck: "1" };
      if (explicitOverrides) {
        env.DEVO_CODING_AGENT_DIR = expected.agentDir;
        env.DEVO_HOME = expected.home;
        env.PI_SKIP_VERSION_CHECK = expected.skipVersionCheck;
      }
      const result = spawnSync(process.execPath, [
        "--import", "tsx", "--input-type=module", "--eval",
        `
          import { existsSync } from "node:fs";
          import { applyDevoAgentDirEnv } from ${JSON.stringify(new URL("./host.ts", import.meta.url).href)};
          const home = ${JSON.stringify(home)};
          const first = applyDevoAgentDirEnv(home);
          const second = applyDevoAgentDirEnv(home);
          console.log(JSON.stringify({
            first, second, exists: existsSync(home),
            agentDir: process.env.DEVO_CODING_AGENT_DIR,
            home: process.env.DEVO_HOME,
            skipVersionCheck: process.env.PI_SKIP_VERSION_CHECK,
          }));
        `,
      ], { env, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout.trim()), {
        first: home, second: home, exists: true, ...expected,
      });
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
