function parseNativeMcpToolName(rawName: string): { server: string; tool: string } | undefined {
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

/** Show MCP calls by tool and server instead of their flattened protocol name. */
export function formatNativeToolTitle(rawName: string): string {
  if (rawName === "ipython") return "Python";
  if (rawName === "toolResult") return "Tool result";

  const parts = parseNativeMcpToolName(rawName);
  if (!parts) return rawName;
  return `MCP · ${humanizeNamePart(parts.tool)} (${humanizeNamePart(parts.server)})`;
}
