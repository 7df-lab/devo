/**
 * Native traffic log for the Devo TUI transport.
 *
 * Mirrors the desktop logger (apps/desktop/src/main/native-traffic-log.ts) so one
 * reader can consume traces from any Devo host: enabled with
 * DEVO_PROTOCOL_TRACE=1, written under DEVO_HOME/traces/, NDJSONL records of
 * { timestamp, direction, kind, id?, method?, payload? }.
 */

import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";

export const PROTOCOL_TRACE_ENV = "DEVO_PROTOCOL_TRACE";
export const PROTOCOL_TRACE_FILE_ENV = "DEVO_PROTOCOL_TRACE_FILE";
export const DEVO_HOME_ENV = "DEVO_HOME";

const TRACES_DIR_NAME = "traces";
const TRACE_FILE_PREFIX = "protocol-";
const TRACE_FILE_SUFFIX = ".ndjsonl";

export const DEFAULT_PREVIEW_LIMIT = 20;
export const MAX_PREVIEW_LIMIT = 50;

export type NativeTrafficDirection = "tui-to-server" | "server-to-tui" | "system";
export type NativeTrafficKind = "request" | "response" | "notification" | "invalid";
export type NativeTrafficJsonRpcId = number | string;

export interface NativeTrafficRecord {
  direction: NativeTrafficDirection;
  kind: NativeTrafficKind;
  id?: NativeTrafficJsonRpcId;
  method?: string;
  payload?: unknown;
}

export interface NativeTrafficLogState {
  enabled: boolean;
  path: string | null;
}

export interface NativeTrafficFileSummary {
  path: string;
  bytes: number;
  modifiedAtMs: number;
}

/**
 * Trace sink shared by the TUI transport and the `/traces` slash command.
 *
 * Implementations must never throw from `record`: tracing is diagnostic and a
 * failed write must not break protocol traffic.
 */
export interface NativeTrafficLog {
  getState(): NativeTrafficLogState;
  record(entry: NativeTrafficRecord): void;
  /** Start tracing at runtime, used by `/traces on`. Idempotent. */
  enable(): NativeTrafficLogState;
  /** Stop tracing at runtime, used by `/traces off`. Keeps the file on disk. */
  disable(): void;
  /** Trace files newest-first, most recent `modifiedAtMs` first. */
  list(): NativeTrafficFileSummary[];
  /** Last `limit` decoded records from the active (or newest) trace file. */
  preview(limit: number): Array<Record<string, unknown>>;
}

export function isProtocolTraceEnabled(value: string | undefined): boolean {
  const normalized = value?.trim();
  if (!normalized) return false;
  return normalized === "1" || normalized.toLowerCase() === "true";
}

export function findDevoHome(env: Record<string, string | undefined>): string {
  const explicit = env[DEVO_HOME_ENV]?.trim();
  if (explicit) {
    const resolved = path.resolve(explicit);
    const stat = statSync(resolved, { throwIfNoEntry: false });
    if (!stat?.isDirectory()) {
      throw new Error(`DEVO_HOME points to ${explicit}, but that path is not a directory`);
    }
    return resolved;
  }
  return path.join(homedir(), ".devo");
}

export function formatProtocolTraceTimestamp(date: Date): string {
  return date.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z");
}

/**
 * Resolve the trace path. `force` bypasses DEVO_PROTOCOL_TRACE so `/traces on`
 * can start tracing in a process that was launched with tracing off.
 */
export function resolveNativeTrafficTracePath(
  options: {
    env?: Record<string, string | undefined>;
    clock?: () => Date;
    pid?: number;
    force?: boolean;
  } = {},
): string | null {
  const env = options.env ?? process.env;
  const clock = options.clock ?? (() => new Date());
  const pid = options.pid ?? process.pid;
  if (!options.force && !isProtocolTraceEnabled(env[PROTOCOL_TRACE_ENV])) return null;

  const explicit = env[PROTOCOL_TRACE_FILE_ENV]?.trim();
  if (explicit) {
    const resolved = path.resolve(explicit);
    mkdirSync(path.dirname(resolved), { recursive: true });
    return resolved;
  }

  const fileName = `${TRACE_FILE_PREFIX}${pid}-${formatProtocolTraceTimestamp(clock())}${TRACE_FILE_SUFFIX}`;
  try {
    const base = path.join(findDevoHome(env), TRACES_DIR_NAME);
    mkdirSync(base, { recursive: true });
    return path.join(base, fileName);
  } catch {
    const base = path.join(tmpdir(), `devo-${TRACES_DIR_NAME}`);
    mkdirSync(base, { recursive: true });
    return path.join(base, fileName);
  }
}

/** Classify one raw NDJSON protocol line into a trace record. */
export function classifyNativeTrafficLine(
  direction: NativeTrafficDirection,
  line: string,
): NativeTrafficRecord {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return { direction, kind: "invalid", payload: line };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { direction, kind: "invalid", payload: parsed };
  }
  const frame = parsed as Record<string, unknown>;
  const method = typeof frame.method === "string" ? frame.method : undefined;
  const rawId = frame.id;
  const id = typeof rawId === "number" || typeof rawId === "string" ? rawId : undefined;
  const kind: NativeTrafficKind =
    method !== undefined
      ? id !== undefined
        ? "request"
        : "notification"
      : id !== undefined
        ? "response"
        : "invalid";
  return { direction, kind, id, method, payload: parsed };
}

/** One `/traces preview` line, e.g. `  3 -> request session/new id=2 118B`. */
export function formatTracePreviewRecord(
  record: Record<string, unknown>,
  index: number,
): string {
  const direction = String(record.direction ?? "");
  const kind = String(record.kind ?? "invalid");
  const method = typeof record.method === "string" ? record.method : "-";
  const id = record.id === undefined ? "-" : String(record.id);
  const payload = record.payload;
  let bytes = 0;
  try {
    bytes = Buffer.byteLength(payload === undefined ? "" : JSON.stringify(payload) ?? "", "utf8");
  } catch {
    bytes = 0;
  }
  const arrow =
    direction === "server-to-tui" ? "<-" : direction === "tui-to-server" ? "->" : "--";
  return `${String(index + 1).padStart(3, " ")} ${arrow} ${kind} ${method} id=${id} ${bytes}B`;
}

class NativeTrafficFileLog implements NativeTrafficLog {
  private state: NativeTrafficLogState;
  private readonly env: Record<string, string | undefined>;
  private readonly clock: () => Date;
  private readonly pid: number;

  constructor(options: {
    env: Record<string, string | undefined>;
    clock: () => Date;
    pid: number;
  }) {
    this.env = options.env;
    this.clock = options.clock;
    this.pid = options.pid;
    const logPath = resolveNativeTrafficTracePath({
      env: this.env,
      clock: this.clock,
      pid: this.pid,
    });
    this.state = { enabled: logPath !== null, path: logPath };
    if (logPath) this.resetFile(logPath);
  }

  getState(): NativeTrafficLogState {
    return { ...this.state };
  }

  enable(): NativeTrafficLogState {
    if (this.state.enabled && this.state.path) return this.getState();
    const logPath = resolveNativeTrafficTracePath({
      env: this.env,
      clock: this.clock,
      pid: this.pid,
      force: true,
    });
    if (!logPath) return this.getState();
    this.resetFile(logPath);
    this.state = { enabled: true, path: logPath };
    return this.getState();
  }

  disable(): void {
    this.state = { enabled: false, path: null };
  }

  record(entry: NativeTrafficRecord): void {
    const target = this.state.path;
    if (!this.state.enabled || !target) return;
    try {
      const line = `${JSON.stringify({ timestamp: this.clock().toISOString(), ...entry })}\n`;
      appendFileSync(target, line, "utf-8");
    } catch {
      // Diagnostic only: never surface a trace write failure into the transport.
      this.state = { enabled: false, path: null };
    }
  }

  list(): NativeTrafficFileSummary[] {
    let names: string[];
    try {
      names = readdirSync(this.traceDir());
    } catch {
      return [];
    }
    return names
      .filter((name) => name.startsWith(TRACE_FILE_PREFIX) && name.endsWith(TRACE_FILE_SUFFIX))
      .flatMap((name) => {
        const filePath = path.join(this.traceDir(), name);
        try {
          const stat = statSync(filePath);
          return [{ path: filePath, bytes: stat.size, modifiedAtMs: stat.mtimeMs }];
        } catch {
          return [];
        }
      })
      .sort((left, right) => right.modifiedAtMs - left.modifiedAtMs);
  }

  preview(limit: number): Array<Record<string, unknown>> {
    const target = this.state.path ?? this.list()[0]?.path;
    if (!target) return [];
    let content: string;
    try {
      content = readFileSync(target, "utf-8");
    } catch {
      return [];
    }
    const lines = content.split("\n").filter((line) => line.trim().length > 0);
    return lines
      .slice(Math.max(0, lines.length - limit))
      .flatMap((line) => {
        try {
          const parsed = JSON.parse(line);
          return parsed && typeof parsed === "object" && !Array.isArray(parsed)
            ? [parsed as Record<string, unknown>]
            : [];
        } catch {
          return [];
        }
      });
  }

  private traceDir(): string {
    if (this.state.path) return path.dirname(this.state.path);
    try {
      return path.join(findDevoHome(this.env), TRACES_DIR_NAME);
    } catch {
      return path.join(tmpdir(), `devo-${TRACES_DIR_NAME}`);
    }
  }

  private resetFile(filePath: string): void {
    try {
      mkdirSync(path.dirname(filePath), { recursive: true });
      writeFileSync(filePath, "", "utf-8");
    } catch {
      // record() disables tracing if the first append fails.
    }
  }
}

/** Build the traffic log for this process. Env is injected so tests stay hermetic. */
export function createNativeTrafficLogFromEnv(
  options: {
    env?: Record<string, string | undefined>;
    clock?: () => Date;
    pid?: number;
  } = {},
): NativeTrafficLog {
  return new NativeTrafficFileLog({
    env: options.env ?? process.env,
    clock: options.clock ?? (() => new Date()),
    pid: options.pid ?? process.pid,
  });
}
