type NativeMcpToolParts = { server: string; tool: string };

function parseNativeMcpToolName(rawName: string): NativeMcpToolParts | undefined {
  const prefix = "mcp__";
  if (!rawName.startsWith(prefix)) return undefined;

  const separator = rawName.indexOf("__", prefix.length);
  if (separator <= prefix.length || rawName.indexOf("__", separator + 2) !== -1) return undefined;

  const server = rawName.slice(prefix.length, separator);
  const tool = rawName.slice(separator + 2);
  const validPart = /^[a-z0-9_]+$/i;
  if (!validPart.test(server) || !validPart.test(tool)) return undefined;
  return { server, tool };
}

function humanizeNamePart(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(" ");
}

/** Present a flattened MCP function name without its internal `mcp__` namespace. */
export function formatNativeToolLabel(rawName: string): string {
  if (rawName === "ipython") return "Python";
  if (rawName === "toolResult") return "Tool result";

  const parts = parseNativeMcpToolName(rawName);
  if (!parts) return rawName;
  return `MCP · ${humanizeNamePart(parts.tool)} (${humanizeNamePart(parts.server)})`;
}
