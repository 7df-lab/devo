/**
 * Devo TUI entry — spawn Native server over stdio and run InteractiveMode.
 */

import { spawn } from "node:child_process";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { createNativeTrafficLogFromEnv } from "./native-traffic-log.js";
import { applyDevoAgentDirEnv, runInteractiveHost } from "./host.js";
import { applyCliSessionOverrides, nativeServerArgs } from "./cli-session-overrides.js";

export function assertInheritedTty(): void {
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    throw new Error(
      "Devo InteractiveMode requires an inherited TTY (exec/replace). Piped sidecars are invalid.",
    );
  }
}

function resolveServerBin(): string {
  return process.env.DEVO_SERVER_BIN || process.env.DEVO_BIN || "devo";
}

export async function main(): Promise<void> {
  assertInheritedTty();
  applyDevoAgentDirEnv();

  const serverBin = resolveServerBin();
  const cwd = process.cwd();
  const child = spawn(serverBin, nativeServerArgs(process.env.DEVO_CLI_LOG_LEVEL), {
    cwd,
    stdio: ["pipe", "pipe", "inherit"],
    env: process.env,
  });

  if (!child.stdin || !child.stdout) {
    throw new Error("failed to open stdio pipes to Devo server");
  }

  const writeLine = (line: string) => {
    child.stdin!.write(`${line}\n`);
  };

  // Create connection and attach stdout BEFORE any RPC awaits so initialize /
  // session/new / subscription/create responses are not lost.
  const conn = NativeAgentConnection.create({
    writeLine,
    cwd,
    trafficLog: createNativeTrafficLogFromEnv(),
  });
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    conn.pushChunk(chunk);
  });

  await conn.initialize({
    name: "devo-tui",
    version: process.env.DEVO_VERSION || "0.1.39",
  });

  const resumeSessionId = String(process.env.DEVO_RESUME_SESSION_ID ?? "").trim();
  if (resumeSessionId) {
    const resumed = await conn.switchSession(resumeSessionId);
    if (resumed.cancelled) {
      throw new Error(`Failed to resume session ${resumeSessionId}`);
    }
  } else {
    await conn.newSession();
  }

  // A CLI --yolo flag must take effect before the first user turn, including
  // when resuming a session with more restrictive saved settings.
  await applyCliSessionOverrides(conn, process.env.DEVO_CLI_FULL_ACCESS);

  const onChildExit = new Promise<never>((_, reject) => {
    child.on("exit", (code, signal) => {
      reject(new Error(`Devo server exited (code=${code}, signal=${signal})`));
    });
    child.on("error", reject);
  });

  try {
    const onboardingOnly = process.env.DEVO_CLI_ONBOARDING_ONLY === "1";
    const result = await Promise.race([
      runInteractiveHost({
        connection: conn,
        cwd,
        forceOnboarding: onboardingOnly,
        exitAfterOnboarding: onboardingOnly,
      }),
      onChildExit,
    ]);
    if (onboardingOnly && !result?.onboardingCompleted) {
      throw new Error("Provider onboarding was not completed.");
    }
  } finally {
    await conn.dispose().catch(() => {});
    // Close our stdio session without killing the singleton server process.
    // Another TUI may be attached via stdio-proxy to the same DEVO_HOME; the
    // Real server stays up until the last proxied client disconnects.
    try {
      child.stdin?.end();
    } catch {
      // ignore
    }
  }
}

const isDirectRun =
  process.argv[1] &&
  (process.argv[1].endsWith("index.ts") ||
    process.argv[1].endsWith("index.js") ||
    process.argv[1].includes(`${"apps"}${"/"}${"tui"}`));

if (isDirectRun || process.env.DEVO_TUI_MAIN === "1") {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}

export { NativeAgentConnection } from "./native-agent-connection.js";
export { runInteractiveHost, createDevoUiServices } from "./host.js";
