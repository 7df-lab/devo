/** Apply explicit CLI flags only after the Native session exists. */
export async function applyCliSessionOverrides(
  connection: { updatePermissionProfile(profile: "fullAccess"): Promise<unknown> },
  fullAccessFlag: string | undefined,
): Promise<void> {
  if (fullAccessFlag === "1") {
    await connection.updatePermissionProfile("fullAccess");
  }
}

/** Forward the CLI's explicit log level to the stdio server subprocess. */
export function nativeServerArgs(logLevel: string | undefined): string[] {
  const level = logLevel?.trim();
  return level && /^(trace|debug|info|warn|error)$/.test(level)
    ? ["--log-level", level, "server", "--transport", "stdio"]
    : ["server", "--transport", "stdio"];
}
