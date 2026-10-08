# Prompt submission and user messages

The Native SDK's `session.promptAsync` returns `{ outcome: "started", turnId }`
when `session/queue/push` starts a turn and supplies its identifier. A queued
prompt returns `{ outcome: "queued", queueItemId }` instead. `turnId` is optional
when the response has no turn object.

An optimistic user item uses that turn identifier. Native user-item
notifications may arrive before the submission response, so the message store
reconciles both arrival orders by turn ID. It keeps identical prompts submitted
in separate turns distinct. Queued prompts stay in the composer queue until
their turn starts.

Accepted steering input is a Native user-message item in the active turn.
It remains a separate user bubble after completion and history reload;
sharing a turn ID with the original prompt does not merge the two bubbles.

## Turn activity

Answers remain visible in the transcript. The outer activity summary collapses
all thoughts and tools in the turn, including activity separated by assistant
commentary. Opening “Worked for 1s” reveals
individually collapsed Thought, Python, and other tool rows. Each row has its
own expand control: opening Thought does not open Python, and vice versa.
The group closes when the working turn finishes.
The outer summary shows Thinking, Running, or Waiting during execution and
elapsed time after completion. Failures, lost connections, and deliberate
stops remain explicit.

Python source and its matching output share one panel, paired by Native `callId`. Unmatched
results remain visible. Long source and output scroll independently, and the
text Copy control copies the complete source. Completed rows omit repeated
Done labels and decorative icons. A normal fresh-kernel notice is omitted;
actionable warnings and provider failures remain in the transcript.

## Model and provider selection

The composer model control and `/model` open the same searchable picker in new
and existing chats. Search matches model names, identifiers, and provider names.
Ready to use shows configured connections and credential-backed providers;
All models also includes providers that need connection. Availability of an
individual model still depends on the provider account.

Unconnected model rows open the existing API-key or desktop OAuth connection
flow. Completing it selects the requested model and refreshes provider data.
Catalog refetches do not restart OAuth or hide a completed connection.
Effort choices retain the server-authored values.

Opening the picker refreshes stale remote catalog data in the background.
Refresh fetches the configured remote source immediately through Native
`model/catalog/refresh`. Saved models remain selectable during a refresh or
network failure, with Retry available. Offline mode is respected.
The list renders 60 matches at a time and supports Show more, arrow keys,
Enter selection, and Escape dismissal.

## Conversation search

The command palette searches Native session summaries as the query changes,
independent of the sidebar's loaded pages. Queries of two or more characters
are debounced by 250 ms and return at most 50 matches. Results merge into the
session store without changing sidebar pagination or live session status.
Stale responses are ignored when the query changes or the palette closes.


## Shared model and provider controls

Settings has one **Models & providers** destination. Its Models view embeds the
same `ModelSelector` used by new and existing chat composers: Ready to use,
All models, search, keyboard selection, inline connection dialogs, reasoning
choices, and server-owned remote refresh share one implementation. Connections
and Model configuration retain provider credentials and advanced model editing.
The old `/settings/models` route remains an alias of this destination.

New-chat defaults in Settings apply to the last selected project (or global
defaults before selecting a project). New chats read Native defaults; local
project agent preferences cannot shadow model or effort choices made in Settings.
Existing chats retain their session-specific selections.

Default model and effort writes use SDK `model.preferences.write`, backed by
Native `model/preferences/write`, without creating a session. SDK writes are
ordered so a rapid model selection followed by effort is validated in that
order. Errors reject the caller, and subsequent selections can retry. Existing
chat changes continue through `session/metadata/update`.

The server projects each model's reasoning capability from the loaded catalog;
listing effort choices does not reload the remote overlay or resolve a complete
turn for every row. Genuine session creation/resume have a 60-second Desktop
transport budget to allow Windows cold startup.
