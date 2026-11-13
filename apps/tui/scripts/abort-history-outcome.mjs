import assert from "node:assert/strict";

/** Bind restored chrome to durable turn identity, not a global literal count. */
export function assertInterruptedHistory(screen, records, sessionId, originalTurnId) {
  function page(method) {
    const request = records.findLast(event => event.direction === "tui-to-server" && event.method === method
      && event.payload?.params?.sessionId === sessionId);
    const response = records.find(event => event.direction === "server-to-tui" && event.kind === "response" && event.id === request?.id);
    assert.ok(request && response?.payload?.result && !response.payload.error, `Missing authoritative ${method} response`);
    assert.ok(!response.payload.result.nextCursor, "Smoke history must fit in one complete page");
    return response.payload.result.data;
  }
  const turns = page("session/turns/list");
  const items = page("session/items/list");
  assert.ok(Array.isArray(turns) && Array.isArray(items));
  assert.equal(turns.find(turn => turn.id === originalTurnId)?.status, "interrupted", "Original turn interruption must be durable");
  const lines = screen.split("\n").map(line => line.trim());
  const boundaries = turns.map(turn => {
    const input = items.find(envelope => envelope.turnId === turn.id && envelope.item?.type === "userMessage");
    const text = input?.item.content?.filter(part => part.type === "text").map(part => part.text).join("");
    if (!text) return null;
    const positions = lines.flatMap((line, index) => line === text ? [index] : []);
    assert.equal(positions.length, 1, `Expected one visible user boundary for ${turn.id}`);
    return { turn, at: positions[0] };
  }).filter(Boolean).sort((a, b) => a.turn.sequence - b.turn.sequence);
  assert.ok(boundaries.some(boundary => boundary.turn.id === originalTurnId));
  let expectedAborts = 0;
  for (let index = 0; index < boundaries.length; index++) {
    const { turn, at } = boundaries[index];
    const end = boundaries[index + 1]?.at ?? lines.length;
    assert.ok(end > at, "Turn boundaries must preserve durable sequence");
    const cell = lines.slice(at + 1, end);
    const count = cell.filter(line => /^Operation aborted(?:\s|$)/.test(line)).length;
    assert.equal(count, turn.status === "interrupted" ? 1 : 0, `Wrong abort count for ${turn.id}`);
    if (turn.status === "interrupted") {
      expectedAborts++;
      const partial = cell.indexOf("ABORT_PARTIAL_MARKER");
      const abort = cell.findIndex(line => /^Operation aborted(?:\s|$)/.test(line));
      const hadPartial = items.some(item => item.turnId === turn.id && item.item?.type === "assistantMessage" && item.item.text?.includes("ABORT_PARTIAL_MARKER"));
      if (hadPartial || turn.id === originalTurnId) {
        assert.equal(cell.filter(line => line === "ABORT_PARTIAL_MARKER").length, 1, `Partial reply duplicated/lost for ${turn.id}`);
        assert.ok(partial < abort, "Abort must follow its partial reply");
      }
    }
  }
  assert.equal(lines.filter(line => /^Operation aborted(?:\s|$)/.test(line)).length, expectedAborts, "Unowned abort marker");
  return expectedAborts;
}
