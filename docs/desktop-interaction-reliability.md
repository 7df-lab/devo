# Desktop queued interaction reliability

Queued Steer uses the Native `session/queue/steer` request with `sessionId`,
`queueItemId`, and `expectedTurnId`. It never reconstructs input from the preview,
resubmits attachments, or follows a steer with a separate remove request.

The server owns admission and its durable receipt. Repeating the same session and
queue identity returns that receipt, including after a lost response, a component
remount, or an empty SDK queue cache. `accepted` refreshes the authoritative
queue instead of assuming the item is absent: it may already have returned as
held recovery input. `keptQueued` also refreshes
the authoritative queue and tells the user that the active turn cannot accept
the message. A server that does not support atomic steering produces
an error; there is no legacy injection/removal fallback. Direct-text `turn/steer`
continues to use its existing behavior.

Pending actions and errors are scoped to their original project and session.
Repeated clicks are locked while a request is pending. Navigation cannot redirect
an old action or place its error on another session. After remount, any repeated
admission request remains safe because deduplication belongs to the server.

Entries marked `recoveryRequired` remain held. They cannot be steered, edited,
reordered, or automatically released. All reordering is disabled while any held
row exists, preventing other rows from crossing its position. Their row warns
that prior effects may have occurred and retrying may repeat them. Only an
explicit **Retry** calls `session/queue/retry`, which creates a fresh queue
identity and publishes the updated queue. The SDK refreshes the queue after
either success or failure. Discard remains
available. No automatic recovery retry is scheduled by the UI or SDK.

List, update, and queued-push responses only update the local cache while their
per-session projection revision remains current. A delayed response cannot
overwrite newer queue notifications or bring back a drained row.

Regression coverage lives in the SDK queue steering tests, composer queue hook
tests, and rendered composer queue row tests. It covers identity-only requests,
lost responses, empty caches, remounts, stale turns, navigation, duplicate clicks,
held rows, explicit recovery, and preservation of direct-text steering.
