/** Own one isolated tmux server for a multi-session smoke test. */
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { basename, join } from "node:path";

export function createOwnedSmokeTmux({ directory, executable, spawn = spawnSync }) {
  // directory is a caller-owned mkdtemp: its random basename is also our socket name.
  const socket = basename(directory);
  const config = join(directory, "tmux.conf");
  writeFileSync(config, "");
  let started = false;
  function mux(args, required = true) {
    const result = spawn(executable, ["-L", socket, "-f", config, ...args], { encoding: "utf8", timeout: 10000 });
    if (required && (result.error || result.status !== 0)) {
      throw new Error(`${args[0]}: ${result.error?.message || result.stderr}`);
    }
    return result;
  }
  return {
    mux,
    start() {
      if (started) throw new Error("Smoke tmux server already started");
      // Mark ownership before creation so finally also cleans a partial startup.
      started = true;
      // Without this live session, the last product session can trigger
      // exit-empty while the next new-session is connecting to the same socket.
      mux(["new-session", "-d", "-s", "smoke-keeper", "--", process.execPath, "-e", "setInterval(() => {}, 60000)"]);
    },
    close() {
      if (!started) return;
      started = false;
      // Only this random socket, never the user's/default tmux server.
      mux(["kill-server"], false);
    },
  };
}
