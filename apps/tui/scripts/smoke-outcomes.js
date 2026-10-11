/** Observable Native outcomes, independent of model reply wording. */
export function rpcSucceeded(records, method) {
  const requests = new Set(records.filter((r) => r.direction === "tui-to-server" && r.method === method).map((r) => r.id));
  return records.some((r) => r.direction === "server-to-tui" && r.kind === "response"
    && requests.has(r.id) && r.payload?.result != null && !r.payload?.error);
}

export function turnOutcome(records, status) {
  const started = new Set(records.filter((r) => r.method === "turn/started").map((r) => r.payload?.params?.turn?.id));
  return records.some((r) => {
    const turn = r.payload?.params?.turn;
    if (r.method !== "turn/completed" || !turn?.id || !started.has(turn.id) || turn.status !== status) return false;
    const items = records.filter((event) => event.method === "item/completed"
      && event.payload?.params?.item?.turnId === turn.id).map((event) => event.payload.params.item.item);
    return items.some((item) => item?.type === "userMessage") && (status === "interrupted"
      || items.some((item) => item?.type === "assistantMessage" && item.text?.trim()
        || item?.type === "toolResult"));
  });
}

export function heartbeatDelivered(records, marker) {
  return records.some((record) => {
    const item = record.payload?.params?.item?.item;
    return record.method === "item/completed" && item?.type === "userMessage"
      && item.content?.some((block) => block.type === "text" && block.text?.includes(marker));
  });
}
