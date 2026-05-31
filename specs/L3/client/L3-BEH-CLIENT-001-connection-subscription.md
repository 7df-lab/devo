---
artifact_id: L3-BEH-CLIENT-001
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-CLIENT-001 — Client Connection, Authentication, and Event Subscription

## Purpose

Define the concrete behavior for establishing a WebSocket connection to the server, authenticating, negotiating protocol compatibility, subscribing to sessions, receiving ordered events, and handling reconnection with catch-up.

## Source Design

L2-DES-APP-003 (Client Server Protocol), L2-DES-CLIENT-001 (Localization Readiness)

## Behavior Specification

### B1. Server Discovery and Connection

- **Trigger**: Client starts (TUI, desktop, IDE plugin).
- **Preconditions**: The endpoint descriptor file may or may not exist.
- **Algorithm / Flow**:
  1. Read the server endpoint descriptor from `<runtime_dir>/server.json` (default: `~/.local/share/devo/runtime/server.json`).
  2. If the file exists and is recent (server started within last 24 hours): extract `websocket_url` and `auth_token`. Attempt connection.
  3. If connection succeeds → proceed to initialization (B2).
  4. If connection fails OR file doesn't exist:
     a. Acquire a startup lock (file lock on `<runtime_dir>/startup.lock`).
     b. Start a detached server process. Pass `--runtime-dir` and other bootstrap args.
     c. Poll for the endpoint descriptor file to appear (max 5 seconds, 100ms interval).
     d. Read the descriptor, connect.
     e. Release the startup lock.
  5. Establish a WebSocket connection to the discovered URL using `tokio-tungstenite`.
- **Postconditions**: A WebSocket connection is open to the local server.
- **Error Handling**: Server fails to start within 5s → report "Server failed to start" and exit or retry. Descriptor file contains invalid JSON → report, attempt to start a new server.

### B2. Client Initialization Handshake

- **Trigger**: WebSocket connection is established.
- **Preconditions**: The socket is open. The client has a unique `client_id` (persisted across restarts).
- **Algorithm / Flow**:
  1. Send `server/initialize` JSON-RPC request:
     - `client_id`: stable UUID v4 generated on first client launch, persisted.
     - `client_kind`: "tui", "desktop", "ide", "browser".
     - `protocol_version`: "1.0" (semver compatible).
     - `auth_token`: from the endpoint descriptor (if found) or empty (for new server).
     - `client_capabilities`: `{ "unicode": true, "ime": true, "images": false }` (TUI-specific).
     - `workspace_root`: current working directory (canonicalized).
  2. Receive response: `server_id`, `server_version`, `protocol_version`, `server_capabilities`, `latest_sequence`.
  3. If protocol version is incompatible (different major): disconnect with an error message to the user.
  4. Store `server_id` and `server_capabilities` for the session.
- **Postconditions**: The client is registered with the server. Subsequent requests are processed.

### B3. Session Subscription and Event Handling

- **Trigger**: Client needs to display a session (new session created or existing session selected).
- **Preconditions**: The client is initialized.
- **Algorithm / Flow**:
  1. Send `session/subscribe`:
     - `session_id`: the target session.
     - `from_sequence`: the last known sequence number (0 for new subscriptions).
     - `event_filter`: optional set of event kinds to receive. Default: all.
     - `projection`: requested projection format.
  2. Receive response: `subscription_id`, optional `session_snapshot`, `next_sequence`.
  3. If `session_snapshot` is provided: populate the local UI state with the snapshot data.
  4. Enter event loop:
     a. Read JSON-RPC notifications from the WebSocket.
     b. For session-scoped and turn-scoped concrete notifications such as `session/started`, `turn/started`, `item/started`, and `item/agentMessage/delta`: extract `seq`, validate monotonic (if out of order, buffer and reorder or request catch-up).
     c. Dispatch to the appropriate UI handler: transcript renderer, tool status, approval modal, plan/goal view, config display.
     d. Update the local `last_sequence` to the highest received `seq`.
  5. Track `subscription_id` for later unsubscribe.
- **Postconditions**: The client receives ordered session events. UI reflects server state.

### B4. Reconnection and Event Catch-Up

- **Trigger**: WebSocket connection drops unexpectedly.
- **Preconditions**: The client has an active subscription. The `last_sequence` is known.
- **Algorithm / Flow**:
  1. Detect disconnection (WebSocket close frame, ping timeout, or TCP error).
  2. Display a reconnection indicator in the UI state area, such as the working indicator row or bottom status line: "Reconnecting...".
  3. Retry connection with exponential backoff: 100ms, 200ms, 400ms, 800ms, 1.6s, 3.2s, 5s (max). Max retries: 20 (~60 seconds total).
  4. On reconnection:
     a. Re-send `server/initialize` (the server may have restarted).
     b. Re-send `session/subscribe` with `from_sequence: last_sequence + 1`.
     c. Receive either:
        - Missed events starting from `from_sequence` (if the server's event buffer still has them).
        - A fresh `session_loaded` snapshot with `latest_sequence` (if buffer was too old).
     d. Reconcile UI state: if snapshot received, replace local state. If missed events received, replay them into local state.
  5. Hide reconnection indicator.
- **Postconditions**: The client is re-subscribed and the UI is consistent with server state.

### B5. Unicode and Localization Safety

- **Trigger**: Any text is received from the server or entered by the user.
- **Preconditions**: The client has declared Unicode capability in initialization.
- **Algorithm / Flow**:
  1. All text from the server is treated as UTF-8. Invalid byte sequences → replacement character (U+FFFD).
  2. Wide characters (CJK, emoji) are accounted for in layout calculations: use `unicode-width` crate for display width.
  3. IME composition: during IME preedit, composer shows the composition string in a distinct style (underlined). Only the committed text is submitted.
  4. Grapheme clusters are handled by the `unicode-segmentation` crate for cursor movement and deletion (delete grapheme, not code point).
- **Postconditions**: All text is displayed correctly regardless of script or language.

### B6. Graceful Disconnect

- **Trigger**: Client exits normally (user presses Ctrl+D on empty composer, or the approved `/exit` command).
- **Preconditions**: The client is connected.
- **Algorithm / Flow**:
  1. Send `session/unsubscribe` for any active subscriptions.
  2. Send `server/shutdown` only if the client is the server owner and user requested server shutdown.
  3. Close the WebSocket connection gracefully (send close frame, wait for close frame).
  4. Persist client state: `last_sequence` per session, `client_id`, recent session list.
  5. Restore terminal to normal mode (disable raw mode, show cursor).
- **Postconditions**: The server knows the client disconnected. Terminal is restored.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-APP-003 | specified-by |
| L2-DES-CLIENT-001 | specified-by |

## Implementation Placement Guidance

- WebSocket client uses `tokio-tungstenite` with `tokio` runtime.
- Reconnection logic belongs in the client connection layer; a conventional placement is `crates/client/src/connection.rs` or the equivalent client transport module.
- The endpoint descriptor file is a JSON file at `<runtime_dir>/server.json` containing `{ "pid": 12345, "websocket_url": "ws://127.0.0.1:PORT/ws", "auth_token": "...", "version": "0.1.0", "started_at": "..." }`.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial client connection, authentication, subscription, reconnection, and event catch-up behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Changed JSON-RPC request examples to slash-separated method names and aligned notification handling with concrete server event method names. |
