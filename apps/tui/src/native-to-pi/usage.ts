import type { CumulativeUsage } from "../../lib/coding-agent/src/core/usage-display.js";

function count(value: unknown): number | null {
  if (typeof value !== "number" && typeof value !== "bigint") return null;
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= 0 ? number : null;
}

/** Native input already includes cache tokens. The provider total is authoritative. */
export function nativeCumulativeUsage(value: unknown, legacyValue?: unknown): CumulativeUsage | undefined {
  if (!value || typeof value !== "object") return undefined;
  const totals = value as Record<string, unknown>;
  const callCount = count(totals.callCount);
  const meteredCallCount = count(totals.meteredCallCount);
  const total = count(totals.totalTokens);
  const legacyOnly = (total ?? 0) > 0 && meteredCallCount === 0;
  const legacyTokens = count((legacyValue as { totalTokens?: unknown } | undefined)?.totalTokens) ?? (legacyOnly ? total : 0) ?? 0;
  const legacy = legacyTokens > 0;
  const unreported = (callCount ?? 0) > 0 && meteredCallCount === 0 && !legacy;
  const unreportedFields = Array.isArray(totals.unreportedFields) ? totals.unreportedFields.filter((field): field is string => typeof field === "string") : undefined;
  const field = (key: string) => unreportedFields?.includes(key) ? null : count(totals[key]);
  const cost = totals.estimatedCost as { micros?: unknown; currency?: unknown } | undefined;
  const micros = count(cost?.micros);
  return {
    tokens: {
      input: unreported || legacyOnly ? null : field("inputTokens"),
      output: unreported || legacyOnly ? null : field("outputTokens"),
      cacheRead: unreported || legacyOnly ? null : field("cacheReadInputTokens"),
      cacheWrite: unreported || legacyOnly ? null : field("cacheCreationInputTokens"),
      total: unreported ? null : field("totalTokens"),
    },
    cost: micros !== null && typeof cost?.currency === "string" ? micros / 1_000_000 : null,
    currency: micros !== null && typeof cost?.currency === "string" ? cost.currency : null,
    ...(legacy ? { legacyTokens } : {}),
    ...(unreportedFields?.length ? { unreportedFields } : {}),
    callCount,
    meteredCallCount,
    coverage: legacy ? "legacy" : unreported ? "unreported"
      : callCount === null || meteredCallCount === null ? "unknown"
      : meteredCallCount < callCount || (unreportedFields?.length ?? 0) > 0 ? "partial"
      : callCount > 0 && !unreportedFields ? "unknown" : "complete",
  };
}

/** Cumulative meters never shrink. Ignore delayed snapshots/replayed older totals. */
export function newerCumulativeUsage(
  current: CumulativeUsage | undefined,
  next: CumulativeUsage | undefined,
): CumulativeUsage | undefined {
  if (!next) return current;
  if (current) {
    if (current.callCount !== null && next.callCount !== null && next.callCount < current.callCount) return current;
    if (current.tokens.total !== null && next.tokens.total !== null && next.tokens.total < current.tokens.total) return current;
  }
  return current?.legacyTokens && !next.legacyTokens
    ? { ...next, legacyTokens: current.legacyTokens, coverage: "legacy" }
    : next;
}

export function unknownCumulativeUsage(): CumulativeUsage {
  return {
    tokens: { input: null, output: null, cacheRead: null, cacheWrite: null, total: null },
    cost: null, currency: null, callCount: null, meteredCallCount: null, coverage: "unknown",
  };
}
