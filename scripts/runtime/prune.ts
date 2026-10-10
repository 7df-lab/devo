import { existsSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";

/** Keep executable modules, licenses, resources and native assets; omit compiler/debug inputs. */
export function pruneNodeInputs(directory: string): void {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      pruneNodeInputs(path);
    } else if (/\.d\.(?:ts|mts|cts)$|\.map$/.test(entry.name) ||
      (/\.(?:ts|mts|cts)$/.test(entry.name) && [".js", ".mjs", ".cjs"].some((ext) => existsSync(path.replace(/\.(?:ts|mts|cts)$/, ext))))) {
      rmSync(path);
    }
  }
}
