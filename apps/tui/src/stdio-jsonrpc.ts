/**
 * Minimal NDJSON JSON-RPC client for Devo Native stdio transport.
 */

export type JsonRpcId = number | string;

export type JsonRpcNotificationHandler = (method: string, params: unknown) => void;

export type JsonRpcServerRequestHandler = (
  id: JsonRpcId,
  method: string,
  params: unknown,
) => void | Promise<void>;

export class StdioJsonRpc {
  private nextId = 1;
  private pending = new Map<
    JsonRpcId,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  private notificationHandlers = new Set<JsonRpcNotificationHandler>();
  private serverRequestHandlers = new Set<JsonRpcServerRequestHandler>();
  private buffer = "";

  constructor(
    private readonly writeLine: (line: string) => void,
    private readonly onIncomingLine?: (line: string) => void,
  ) {}

  pushChunk(chunk: string): void {
    this.buffer += chunk;
    let idx: number;
    while ((idx = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, idx);
      this.buffer = this.buffer.slice(idx + 1);
      this.handleIncomingLine(line);
    }
  }

  handleIncomingLine(line: string): void {
    this.onIncomingLine?.(line);
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      return;
    }

    const id = msg.id as JsonRpcId | undefined;
    const method = typeof msg.method === "string" ? msg.method : undefined;

    // Server → client request (reverse-RPC): has method + id
    if (method !== undefined && id !== undefined && msg.result === undefined && msg.error === undefined) {
      for (const handler of this.serverRequestHandlers) {
        try {
          const out = handler(id, method, msg.params);
          if (out && typeof (out as Promise<void>).then === "function") {
            (out as Promise<void>).catch(() => {});
          }
        } catch {
          // ignore
        }
      }
      return;
    }

    // Notification: method, no id
    if (method !== undefined && id === undefined) {
      for (const handler of this.notificationHandlers) {
        try {
          handler(method, msg.params);
        } catch {
          // ignore
        }
      }
      return;
    }

    // Response
    if (id !== undefined && this.pending.has(id)) {
      const pending = this.pending.get(id)!;
      this.pending.delete(id);
      if (msg.error) {
        const err = msg.error as { code?: string | number; message?: string };
        // Keep the protocol error code reachable (e.g. ActiveTurnNotSteerable)
        // so callers can branch on it instead of parsing message text.
        const error = new Error(err.message ?? JSON.stringify(msg.error)) as Error & {
          code?: string | number;
        };
        if (err.code !== undefined) error.code = err.code;
        pending.reject(error);
      } else {
        pending.resolve(msg.result);
      }
    }
  }

  onNotification(handler: JsonRpcNotificationHandler): () => void {
    this.notificationHandlers.add(handler);
    return () => this.notificationHandlers.delete(handler);
  }

  onServerRequest(handler: JsonRpcServerRequestHandler): () => void {
    this.serverRequestHandlers.add(handler);
    return () => this.serverRequestHandlers.delete(handler);
  }

  request(method: string, params?: unknown): Promise<unknown> {
    const id = this.nextId++;
    const payload = {
      jsonrpc: "2.0",
      id,
      method,
      params: params ?? {},
    };
    const result = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    this.writeLine(JSON.stringify(payload));
    return result;
  }

  notify(method: string, params?: unknown): void {
    this.writeLine(JSON.stringify({ jsonrpc: "2.0", method, params: params ?? {} }));
  }

  respond(id: JsonRpcId, result: unknown): void {
    this.writeLine(JSON.stringify({ jsonrpc: "2.0", id, result }));
  }

  respondError(id: JsonRpcId, message: string, code = -32000): void {
    this.writeLine(
      JSON.stringify({
        jsonrpc: "2.0",
        id,
        error: { code, message },
      }),
    );
  }
}
