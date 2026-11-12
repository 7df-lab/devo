/** Interactive TUI admission, not a sandbox or a server authorization boundary. */
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, realpath, rename, stat, unlink, writeFile } from "node:fs/promises";
import { isAbsolute, join, relative, sep } from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import type { StdioJsonRpc } from "./stdio-jsonrpc.js";

export type TrustPrompt = (cwd: string) => Promise<boolean>;

export class WorkspaceTrust {
  private accepted = new Set<string>();
  private prompt?: TrustPrompt;

  constructor(private readonly home: string, prompt: TrustPrompt) {
    this.prompt = prompt;
  }

  /** Once the fullscreen UI owns stdin, never open a competing readline prompt. */
  finishStartup(): void {
    this.prompt = undefined;
  }

  async require(cwd: string): Promise<string> {
    const canonical = await realpath(cwd);
    if (!(await stat(canonical)).isDirectory()) throw new Error("Workspace is not a directory");
    if (this.accepted.has(canonical)) return canonical;
    // A repository must not be able to ship its own trust approval. A DEVO_HOME
    // inside the selected workspace is supported, but trust is session-only.
    const canonicalHome = await realpath(this.home);
    const rel = relative(canonical, canonicalHome);
    const persist = rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel);
    const directory = join(canonicalHome, "workspace-trust");
    const key = createHash("sha256").update(canonical).digest("hex");
    const file = join(directory, `${key}.json`);
    if (persist) {
      try {
        const record = JSON.parse(await readFile(file, "utf8"));
        if (record.version === 1 && record.cwd === canonical && record.trusted === true) {
          this.accepted.add(canonical);
          return canonical;
        }
      } catch {
        // Missing, corrupt, and unreadable records are never implicit consent.
      }
    }
    if (!this.prompt) {
      throw new Error(`Workspace is not trusted: ${JSON.stringify(canonical)}. Exit and start devo in that directory to review the trust prompt.`);
    }
    if (!(await this.prompt(canonical))) throw new Error("Workspace not trusted. Devo did not open the session.");
    if (await realpath(cwd) !== canonical) {
      throw new Error("Workspace changed while awaiting trust. Restart Devo and review the new directory.");
    }
    if (persist) {
      await mkdir(directory, { recursive: true, mode: 0o700 });
      const temporary = `${file}.${randomUUID()}.tmp`;
      try {
        await writeFile(temporary, JSON.stringify({ version: 1, cwd: canonical, trusted: true }), { flag: "wx", mode: 0o600 });
        await rename(temporary, file);
      } finally {
        await unlink(temporary).catch(() => {});
      }
    }
    this.accepted.add(canonical);
    return canonical;
  }
}

export function promptWorkspaceTrust(cwd: string, input: Readable = process.stdin, output: Writable = process.stdout): Promise<boolean> {
  output.write(`\n┌─ Workspace trust\n│ ${JSON.stringify(cwd)}\n│\n│ Only continue if you trust this folder's contents.\n│ Devo may read project instructions and configuration, contact model\n│ services, and run tools or MCP servers with your account's access.\n│ Trust does not enable a sandbox or change your permission settings.\n└─ Trust this workspace? Type yes to continue; Enter to exit: `);
  return new Promise((resolve) => {
    const reader = createInterface({ input, output, terminal: false });
    reader.once("line", (line) => {
      resolve(line.trim().toLowerCase() === "yes");
      reader.close();
    });
    reader.once("close", () => resolve(false));
    reader.once("SIGINT", () => { reader.close(); resolve(false); });
  });
}

/** Read-only lookup before a saved session can load project configuration. */
export async function requireSessionWorkspace(rpc: StdioJsonRpc, trust: WorkspaceTrust, sessionId: string): Promise<void> {
  const result = await rpc.request("session/read", { sessionId }) as { session?: { cwd?: unknown } };
  if (typeof result.session?.cwd !== "string" || !result.session.cwd) throw new Error("Cannot verify the session workspace");
  await trust.require(result.session.cwd);
}
