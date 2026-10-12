# TUI context usage

The footer uses Native occupancy's effective context window. It can differ
from the raw model catalog limit, and must not be replaced by a client default
when stats refresh, catalog enrichment or session resume completes.

Before Native occupancy is available, the model window supplies the initial
zero-token footer. An explicit model switch updates that provisional window
immediately: an immediate stats read may still describe the previous model.
The next Native usage notification with a known window ends that provisional
state and supplies the authoritative effective window.

Token-only notifications retain the last known window and recalculate the
percentage. The connection emits that resolved value to the UI as well as
storing it; unknown-window sentinel values must not replace known capacity.
New-session and resume transitions reset the provisional state appropriately.

Regression coverage in native-agent-connection.test.ts checks authoritative
stats through catalog enrichment, model-switch behavior through fresh Native
usage, and partial usage notifications in both connection state and UI events.
Ctrl+O detail cycling and model selection commands keep their existing behavior.
