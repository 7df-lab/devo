/** Renderer-friendly results shared by live and restored Native tools. */

/** Hosted items contain both call and result; terminal items can omit output. */
export function shapeHostedToolResult(item: Record<string, unknown>, state: unknown) {
  const hasOutput = item.output != null || item.displayContent != null || item.display_content != null;
  const terminal = ["completed", "failed", "interrupted", "lost"].includes(String(state));
  if (!hasOutput && !terminal) return undefined;
  const isError = Boolean(item.isError ?? item.is_error) || ["failed", "interrupted", "lost"].includes(String(state));
  const display = item.displayContent ?? item.display_content ??
    (hasOutput ? undefined : isError ? `Tool ${state}` : "No output returned");
  return { ...shapeToolResultOutput(item.output, display), isError };
}

function isIpythonDetailsShape(value: Record<string, unknown>): boolean {
  return (
    typeof value.status === "string" ||
    typeof value.durationMs === "number" ||
    typeof value.stdout === "string" ||
    typeof value.stderr === "string" ||
    typeof value.errorName === "string" ||
    typeof value.errorEname === "string" ||
    (value.error != null && typeof value.error === "object")
  );
}

function normalizeIpythonDetails(raw: Record<string, unknown>): Record<string, unknown> {
  const errorName =
    typeof raw.errorEname === "string"
      ? raw.errorEname
      : typeof raw.errorName === "string"
        ? raw.errorName
        : undefined;
  const errorValue =
    typeof raw.errorValue === "string"
      ? raw.errorValue
      : typeof (raw as { evalue?: unknown }).evalue === "string"
        ? String((raw as { evalue: string }).evalue)
        : "";
  let error = raw.error;
  if ((!error || typeof error !== "object") && errorName) {
    error = {
      ename: errorName,
      evalue: errorValue,
      traceback: Array.isArray(raw.traceback) ? raw.traceback : [],
    };
  }
  return {
    ...raw,
    ...(errorName ? { errorEname: errorName } : {}),
    ...(error ? { error } : {}),
  };
}

function ipythonResultText(record: Record<string, unknown>): string {
  if (typeof record.output === "string" && record.output.length > 0) {
    return record.output;
  }
  const parts = [record.stdout, record.result, record.stderr].filter(
    (value): value is string => typeof value === "string" && value.length > 0,
  );
  if (parts.length > 0) return parts.join("\n");
  if (typeof record.status === "string") return `(status: ${record.status})`;
  return "";
}

/** One-line / short preview for hosted web_search hit arrays (never JSON.stringify the dump). */
export function summarizeWebSearchOutput(output: unknown): string {
  if (Array.isArray(output)) {
    const titles = output
      .slice(0, 5)
      .map((hit) => {
        if (!hit || typeof hit !== "object") return "";
        const record = hit as Record<string, unknown>;
        return String(record.title ?? record.name ?? record.url ?? "").trim();
      })
      .filter(Boolean);
    const count = output.length;
    const noun = count === 1 ? "result" : "results";
    if (titles.length === 0) return `${count} search ${noun}`;
    const more = count > titles.length ? ` (+${count - titles.length} more)` : "";
    return `${count} ${noun}: ${titles.join("; ")}${more}`;
  }
  if (output && typeof output === "object") {
    const record = output as Record<string, unknown>;
    if (Array.isArray(record.results)) return summarizeWebSearchOutput(record.results);
    if (Array.isArray(record.hits)) return summarizeWebSearchOutput(record.hits);
    if (typeof record.output === "string" && record.output.trim()) {
      return truncateDisplayText(record.output, 400);
    }
    if (typeof record.text === "string" && record.text.trim()) {
      return truncateDisplayText(record.text, 400);
    }
  }
  if (typeof output === "string") return truncateDisplayText(output, 400);
  return "";
}

function truncateDisplayText(text: string, maxChars: number): string {
  const trimmed = text.replace(/\s+/g, " ").trim();
  if (trimmed.length <= maxChars) return trimmed;
  return `${trimmed.slice(0, Math.max(0, maxChars - 1)).trimEnd()}…`;
}

/**
 * Kernel shell/pty results arrive as the model-visible envelope dict — the
 * human-readable output lives in its `output` string and every other key is
 * execution metadata (exit, duration_ms, …).
 */
export function isShellResultEnvelope(record: Record<string, unknown>): boolean {
  return (
    typeof record.command === "string" &&
    (typeof record.exit === "number" || typeof record.exitCode === "number") &&
    (typeof record.output === "string" || record.output == null)
  );
}

/** Decode a kernel shell envelope into bash-renderer content + metadata details. */
export function shellResultFromEnvelope(record: Record<string, unknown>): {
  content: Array<{ type: string; text: string }>;
  details: Record<string, unknown>;
} {
  const output = typeof record.output === "string" ? record.output : "";
  const exit = typeof record.exit === "number" ? record.exit : record.exitCode;
  return {
    content: [{ type: "text", text: output || "(no output)" }],
    details: {
      exitCode: typeof exit === "number" ? exit : undefined,
      durationMs: typeof record.duration_ms === "number" ? record.duration_ms : undefined,
      command: record.command,
    },
  };
}

function displayContentText(displayContent: unknown): string {
  if (typeof displayContent === "string") return displayContent;
  if (Array.isArray(displayContent)) {
    return displayContent
      .map((part) =>
        part && typeof part === "object" && typeof (part as { text?: unknown }).text === "string"
          ? String((part as { text: string }).text)
          : "",
      )
      .filter(Boolean)
      .join("\n");
  }
  return "";
}

/** Normalize Native tool result output into ToolResultMessage fields. */
export function shapeToolResultOutput(
  output: unknown,
  displayContent?: unknown,
): { content: Array<{ type: string; text: string }>; details?: Record<string, unknown> } {
  const withContent = (value: Record<string, unknown>, fallbackText = "") => {
    if (Array.isArray(value.content)) {
      return value as { content: Array<{ type: string; text: string }>; details?: Record<string, unknown> };
    }
    if (typeof value.content === "string") {
      return { ...value, content: [{ type: "text", text: value.content }] };
    }
    const text =
      typeof value.text === "string"
        ? value.text
        : typeof value.message === "string"
          ? value.message
          : typeof value.output === "string"
            ? value.output
            : fallbackText || truncateDisplayText(JSON.stringify(value), 800);
    return { ...value, content: [{ type: "text", text }] };
  };

  // Hosted web_search often ships a raw hit array as `output`. Never stringify it
  // into the tool row — keep a short summary in content and the raw hits in details.
  if (Array.isArray(output)) {
    if (output.length > 0 && output.every((part) =>
      part && typeof part === "object" && part.type === "text" && typeof part.text === "string"
    )) {
      return { content: output };
    }
    const summary =
      summarizeWebSearchOutput(output) ||
      displayContentText(displayContent) ||
      `${output.length} results`;
    return {
      content: [{ type: "text", text: summary }],
      details: { resultKind: "web_search", hits: output, hitCount: output.length },
    };
  }

  if (output != null && typeof output === "object") {
    const record = output as Record<string, unknown>;
    if (record.details != null && typeof record.details === "object") {
      const details = normalizeIpythonDetails(record.details as Record<string, unknown>);
      return withContent({ ...record, details });
    }
    if (Array.isArray(record.results) || Array.isArray(record.hits)) {
      const hits = (Array.isArray(record.results) ? record.results : record.hits) as unknown[];
      return {
        content: [{ type: "text", text: summarizeWebSearchOutput(hits) }],
        details: { ...record, resultKind: "web_search", hits, hitCount: hits.length },
      };
    }
    // Anthropic web_fetch wraps the fetched page in a document block. Render
    // the page text, not its transport envelope (or encoded binary documents).
    const document = record.type === "web_fetch_result" ? record.content : record;
    if (document && typeof document === "object" && !Array.isArray(document)) {
      const doc = document as Record<string, unknown>;
      const source = doc.source as Record<string, unknown> | undefined;
      if (doc.type === "document" && source?.type === "text" && typeof source.data === "string") {
        const text = [doc.title, source.data].filter((part) => typeof part === "string" && part).join("\n");
        return {
          content: [{ type: "text", text }],
          details: { resultKind: "web_fetch", url: record.url },
        };
      }
    }
    if (isIpythonDetailsShape(record) && !Array.isArray(record.content)) {
      return {
        content: [{ type: "text", text: ipythonResultText(record) }],
        details: normalizeIpythonDetails(record),
      };
    }
    // Kernel shell results: decode the envelope instead of stringifying it
    // into the tool row (the raw JSON dump is never what a reader wants).
    if (isShellResultEnvelope(record)) {
      return shellResultFromEnvelope(record);
    }
    return withContent(record, displayContentText(displayContent));
  }
  if (typeof output === "string") {
    const display = displayContentText(displayContent);
    // Hosted / skill web_search sometimes arrives as a JSON-encoded hit array string.
    const trimmed = output.trim();
    if (trimmed.startsWith("[") || trimmed.startsWith("{")) {
      try {
        const parsed = JSON.parse(trimmed) as unknown;
        if (parsed != null && typeof parsed === "object") {
          const record = parsed as Record<string, unknown>;
          if (Array.isArray(parsed) || Array.isArray(record.results) || Array.isArray(record.hits) ||
            isShellResultEnvelope(record) || record.type === "web_fetch_result" || record.type === "document" ||
            typeof record.text === "string" || typeof record.output === "string") {
            return shapeToolResultOutput(parsed, displayContent);
          }
        }
      } catch {
        // not JSON — fall through
      }
    }
    // Prefer a short display_content when the raw string is a huge dump.
    if (display && output.length > 800 && display.length < output.length) {
      return {
        content: [{ type: "text", text: display }],
        details: { resultKind: "truncated_output", fullText: output },
      };
    }
    if (output.length > 2_000) {
      return {
        content: [{ type: "text", text: truncateDisplayText(output, 400) }],
        details: { resultKind: "truncated_output", fullText: output },
      };
    }
    return { content: [{ type: "text", text: output }] };
  }
  if (displayContent != null) {
    if (Array.isArray(displayContent)) {
      return { content: displayContent as Array<{ type: string; text: string }> };
    }
    if (typeof displayContent === "string") {
      return { content: [{ type: "text", text: displayContent }] };
    }
    if (typeof displayContent === "object") {
      return withContent(displayContent as Record<string, unknown>);
    }
  }
  return { content: [{ type: "text", text: "" }] };
}
