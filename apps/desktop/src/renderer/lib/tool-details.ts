type NativeItemRecord = Record<string, unknown>;

function asRecord(value: unknown): NativeItemRecord | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as NativeItemRecord)
    : undefined;
}

function textFromToolOutput(output: unknown): string | undefined {
  const outputRecord = asRecord(output);
  if (!outputRecord) return undefined;

  const content = outputRecord.content;
  if (Array.isArray(content)) {
    const textItems = content
      .map(asRecord)
      .filter((item): item is NativeItemRecord => item?.type === "text" && typeof item.text === "string")
      .map((item) => item.text as string);
    if (textItems.length > 0) return textItems.join("\n");
  }

  const details = asRecord(outputRecord.details);
  if (typeof details?.stdout === "string" && details.stdout.length > 0) return details.stdout;
  if (typeof details?.stderr === "string" && details.stderr.length > 0) return details.stderr;
  return undefined;
}

/** Return source code for an RLM Python tool call, when present. */
export function getNativePythonToolCode(rawItem: unknown): string | undefined {
  const item = asRecord(rawItem);
  if (!item || item.type !== "toolCall" || item.toolName !== "ipython") return undefined;
  const input = asRecord(item.input) ?? asRecord(item.arguments);
  return typeof input?.code === "string" ? input.code : undefined;
}

/** Render common Native tool payloads as readable code or output, not escaped JSON. */
export function formatNativeToolDetails(rawItem: unknown): string {
  const item = asRecord(rawItem);
  if (!item) return typeof rawItem === "string" ? rawItem : (JSON.stringify(rawItem, null, 2) ?? "");

  if (typeof item.output === "string") return item.output;
  if (typeof item.displayContent === "string") return item.displayContent;

  const pythonCode = getNativePythonToolCode(item);
  if (pythonCode !== undefined) return pythonCode;

  if (item.type === "toolResult") {
    const outputText = textFromToolOutput(item.output);
    if (outputText !== undefined) return outputText;
  }

  const fallback = JSON.stringify(item.input ?? item, null, 2);
  return fallback ?? "";
}
