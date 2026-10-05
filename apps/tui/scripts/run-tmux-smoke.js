/**
 * Minimal tmux/psmux smoke runner for apps/tui.
 * Markers only — do not assert exact model text.
 */

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");
const isWin = process.platform === "win32";
const script = isWin
  ? join(__dirname, "tmux-tui-smoke.ps1")
  : join(__dirname, "tmux-tui-smoke.sh");

if (!existsSync(script)) {
  console.error(`smoke script missing: ${script}`);
  process.exit(1);
}

const args = process.argv.slice(2);
const result = isWin
  ? spawnSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], {
      cwd: root,
      stdio: "inherit",
      env: process.env,
    })
  : spawnSync("bash", [script, ...args], {
      cwd: root,
      stdio: "inherit",
      env: process.env,
    });

process.exit(result.status ?? 1);
