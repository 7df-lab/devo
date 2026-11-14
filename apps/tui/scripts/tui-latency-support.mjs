/** Statistics and observation helpers for the real product PTY benchmark. */
// /new takes optional arguments: bare-name Enter accepts completion, not submission.
export const NEW_SESSION_INPUT = "/new ";

export const metricNames = ["first_trust_screen_ms", "accepted_to_editable_ms", "input_echo_ms", "new_session_editable_ms", "resume_empty_editable_ms"];

export function positiveInteger(value, name) {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < 1) throw new Error(`${name} must be a positive integer`);
  return number;
}

export function parseTrace(text) {
  // The writer may still be appending the last record.
  return text.split("\n").slice(0, -1).filter(Boolean).map(JSON.parse);
}

export function sessionResult(records, method, expectedId) {
  const requests = new Set(records.filter(r => r.direction === "tui-to-server" && r.method === method
    && (expectedId === undefined || r.payload?.params?.sessionId === expectedId)).map(r => r.id));
  return records.find(r => r.direction === "server-to-tui" && r.kind === "response" && requests.has(r.id)
    && !r.payload?.error && typeof r.payload?.result?.session?.id === "string"
    && (expectedId === undefined || r.payload.result.session.id === expectedId))?.payload.result.session.id;
}

export function summarize(samples) {
  if (!samples.length) throw new Error("Cannot summarize an empty run");
  return Object.fromEntries(metricNames.map(name => {
    const sorted = samples.map(sample => sample[name]).sort((a, b) => a - b);
    if (sorted.some(value => !Number.isFinite(value) || value < 0)) throw new Error(`Invalid metric: ${name}`);
    const quantile = p => sorted[Math.ceil(p * sorted.length) - 1];
    return [name, { count: sorted.length, min: sorted[0], p50: quantile(0.5), p95: quantile(0.95), max: sorted.at(-1) }];
  }));
}

export async function waitForObservation({ label, observe, predicate, now, sleep, timeoutMs, pollMs }) {
  const start = now();
  do {
    const observation = observe();
    if (predicate(observation)) return observation;
    if (now() - start >= timeoutMs) break;
    await sleep(pollMs);
  } while (now() - start < timeoutMs);
  throw new Error(`Timed out: ${label}`);
}
