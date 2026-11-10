# MCP connection lifecycle

## Ownership and current limits

Devo currently has two distinct MCP paths. They are not interchangeable:

| Path | Connection owner | Credentials | Result contract |
| --- | --- | --- | --- |
| `rlm.mcp` | Rust session MCP manager | Native runtime credential/config store | Full MCP `CallToolResult` |
| Bundled `linear` / `notion` | Authored Python integration | Login host's `auth.json` | Parsed structured content or text |

A same-named user server must never repoint an authored integration or receive
its grant. This change does not migrate credentials between stores, start login,
or add a second OAuth implementation to Rust.

The Native host previously acknowledged `mcp.refresh` and `mcp.config` with
`{"servers": []}`. That did not refresh a token or return a valid authored
configuration. It now rejects these unsupported contracts explicitly. Calling
Rust `McpManager::refresh` in their place would be incorrect: that method refreshes
native server discovery, not the Python integration's `auth.json`.

## Native discovery and status

```python
from rlm import mcp

# Observed state only. Does not launch servers or expose credentials/config.
print(await mcp.list_servers())

# Verify one configured server's connection and obtain current schemas.
tools = await mcp.list_tools("docs")

# Uses the existing host approval path. Never retried through Python.
result = await mcp.call_tool("docs", "search", {"query": "release"})
```

`list_servers()` returns `server`, `status`, `authStatus`, and `toolCount`.
Statuses are `disabled`, `not_started`, `starting`, `ready`, `failed`,
`auth_required`, `degraded`, or `stopped`. `ready` is the manager's observed
successful initialization/discovery, not proof of current network health.
Token presence alone is not a successful connection. Use `list_tools()` for a
fresh check; even a successful refresh result must be in `ready` state. A ready
server with zero tools is valid.

The status reply intentionally excludes URLs, commands, environment variables,
headers, tool schemas, and credential values. Tool calls still pass through the
existing permission and plan-mode checks. Discovery never retries a mutation.

## Authored integrations

`await linear.list_tools()` and `await notion.list_tools()` now verify discovery
on every explicit call. Dynamic tool methods reuse schemas between explicit
checks; a failed check clears them rather than presenting stale discovery as a
current connection. Tool calls keep their parsed-result contract and are never
automatically replayed after a transport failure.

Token resolution distinguishes:

- Missing grant: `NotEnabled`, with login instructions.
- Expired/malformed grant: ask its login host to refresh; serialize concurrent
  refresh attempts and re-read the persisted grant before use.
- Host failure or an acknowledged no-op: actionable refresh error, not a false
  “no credentials” diagnosis. Provider error bodies are not copied into the
  raised traceback.
- Explicit endpoint mismatch: refuse use and refresh before sending credentials.
- Cancellation: propagate without retrying or retaining the refresh lock.

Legacy built-in grants without endpoint metadata remain readable for backwards
compatibility. Their endpoint is fixed in the authored integration. Explicitly
bound grants must match it exactly. Existing static-token environment support is
unchanged; this change does not copy credentials into host replies.

## Verification

The regression suite covers both contracts and failure cases:

```sh
PYTHONPATH=crates/kernel/rlm-runtime/src python -m unittest discover \
  -s crates/kernel/rlm-runtime/tests -p 'test_mcp*.py' -v
cargo test -p devo-server --lib kernel_host_bridge
cargo test -p devo-server --lib mcp_real_kernel -- --ignored
```

The Python wire test launches the actual REPL and supplies synthetic host
replies. The Rust integration test launches that same REPL through
`KernelSession`, uses the real host dispatcher/approval bridge, and substitutes
only the MCP manager and expired credential fixture. Neither test contacts
Linear/Notion or verifies a real OAuth browser flow.

## Remaining complete-connection work

This is a correctness and diagnostics slice, not an end-to-end OAuth migration.
Completing the experience requires a single chosen credential/connection owner
for login, refresh, logout, settings, and kernel calls; native login/cancellation
and reconnect UI; endpoint-bound credential migration; and live provider tests
with explicit user authorization. Reuse the existing Native protocol instead of
adding parallel provider-specific transports or a copied plugin catalog.

The useful Prime comparison is its separation of stored credentials from a
verified connection record (`catalog_status_views.rs`, Prime `74591cd`). Devo
can reuse its existing manager status rather than import that catalog's entire
architecture.

## Native transport reconnect

`mcp/reconnect` accepts exactly `{ "name": "configured-server-id" }`. Desktop
Settings → MCP exposes this as **Reconnect**. It is an explicit retry, not an
OAuth login flow: the operation replaces that configured Native transport,
performs a fresh handshake, and discovers its tools using the existing Native
credential store. It does not accept credentials, change the configured endpoint,
write authored integration `auth.json` grants, or invoke/replay a tool call.

The configured process manager is the target. Contexts sharing that manager receive
the selected server's updated schemas while retaining their own host tools,
allowlists, deferred tool state, and execution stores. A distinct workspace MCP
manager, including a same-named server override, is not reconnected. Explicit
session registry overrides are updated within their actor, rather than replaced
with the process-wide registry. An independently owned explicit registry, such as
an ACP session registry, retains its own manager even when its session context
shares the process manager or its current discovery is empty.

Old selected-server schemas are withdrawn before reconnect. Success publishes the
manager's current verified metadata, including visibility and read-only hints.
Failure removes stale schemas and reports a redacted retryable error; `mcp/list`
remains the authoritative observed status. Neither stored tokens nor successful
RPC acknowledgement alone imply the server is ready. Desktop reloads status and
tools after success or failure rather than keeping an optimistic connected flag.

A concurrent reconnect is rejected. Manager discovery has per-server admission
and generation checks, with no transport I/O under state locks. Ordinary discovery
waits for an in-flight discovery and receives its complete metadata snapshot,
rather than publishing an incomplete registry because the server was busy. Status remains
readable during I/O, and disable invalidates the in-flight generation and wins
over late discovery results. Already executing calls may complete on their
previously acquired transport; reconnect does not repeat them. Cancellation
releases admission and allows a later explicit retry.

Enable/disable registry updates use the same targeted publication path, without
rediscovering unrelated servers. Global enablement changes do not overwrite a
same-named workspace-specific server record, even when its current values equal
the global defaults. Workspace loads that overlap invalidation are retried before
publication, so a late load cannot restore pre-disable state. Native login UI, authored grant
migration, provider OAuth rotation, and live Linear/Notion verification remain
separate work and are not claimed by this transport reconnect slice.
