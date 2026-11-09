# Opening a saved session in Desktop

Child sessions do not appear in the root-only sidebar discovery. Opening a child
from the Subagents roster or navigating directly to a saved session may therefore
need an exact-ID metadata read before the normal conversation view can mount.

## Error and retry contract

Desktop uses the existing Native `session/read` through `session.get`. This read
never scans the global roster, resumes model work, starts a turn, or loads a
transcript. Once the identity is available, the existing conversation view loads
its history as usual; explicit turn recovery remains a separate user action.

- A Native SessionNotFound response is the only read failure represented as a
  missing session. Permission, transport and other runtime errors propagate to
  an accessible **Could not open session** state with their error message.
- A disconnected runtime has its own **Runtime disconnected** state. Unknown
  identities are checked again when the connected runtime returns. Missing base
  clients are errors, not evidence that the child was deleted.
- **Try again** performs another exact-ID read for a missing or failed session.
  Repeated clicks share the current in-flight request. There is no periodic
  retry loop, global-scan compatibility path, or automatic turn continuation.
- Only visible, unknown, nonempty session IDs need discovery. Known identities
  use the store, and inactive kept-alive panels do not start reads.
- Requests already sent may finish after navigation, hiding, disconnect, server
  switch or unmount. Their obsolete responses cannot populate the current store
  or replace the current error. An identity arriving through a live event wins
  over an older discovery snapshot, even before React has rerendered.
- Routed session panels and the reusable embedded SessionView share the same
  loading/offline/missing/error presentation and retry behavior. This changes
  identity resolution, not transcript loading or normal session controls.

## Verification

Service tests exercise the real SDK's exact Native read and distinguish missing,
connection and permission errors without resume/history requests. Hook/component
regressions cover reconnect, explicit repeated retry, missing identities, inactive
and known sessions, changed IDs, server switches, disconnect during a read, live
updates racing snapshots, mismatched response IDs, unmount, suspended navigation and keep-alive return.
A connection-manager test verifies that no base client is reported as a connection failure
rather than as a missing child.
