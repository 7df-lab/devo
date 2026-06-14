# devo-client

`devo-client` contains client transports for talking to the Devo runtime server.
It exposes a stdio client that spawns a server process, sends JSON
request/notification messages over stdin, and reads responses/events from
stdout.

## Public Interfaces

- `StdioServerClientConfig`: spawn configuration for the stdio server process,
  including the program path, optional workspace root, and extra arguments.
- `ServerNotificationMessage`: raw server notification with a method name and
  JSON params.
- `StdioServerClient`: async stdio transport client. It owns the child process,
  request routing, notification stream, and shutdown path.

## StdioServerClient Methods

- `spawn`: start the server process and attach stdin/stdout/stderr readers.
- `initialize`: perform the protocol handshake and send the `initialized`
  notification.
- `session_start`, `session_resume`, `session_list`: create, resume, and list
  sessions.
- `session_title_update`, `session_metadata_update`,
  `session_permissions_update`: update session metadata shown by clients.
- `session_compact`, `session_fork`, `session_rollback`: manage session history
  and derived sessions.
- `agent_list`, `agent_spawn`, `agent_close`: inspect and manage background
  agents.
- `goal_create`, `goal_set`, `goal_status`, `goal_pause`, `goal_resume`,
  `goal_complete`, `goal_clear`: manage long-running goal state.
- `skills_list`, `skills_changed`, `skills_set_enabled`: read and update skill
  catalog state.
- `model_catalog`, `model_saved`: read available and saved model information.
- `provider_vendor_list`, `provider_vendor_upsert`, `provider_validate`: manage
  provider configuration.
- `command_exec`, `command_exec_write`, `command_exec_resize`,
  `command_exec_terminate`: control server-side command execution.
- `turn_start`, `turn_shell_command`, `turn_interrupt`, `turn_steer`: drive and
  steer agent turns.
- `approval_respond`, `request_user_input_respond`: answer pending server
  prompts.
- `reference_search_start`, `reference_search_update`,
  `reference_search_cancel`: control reference search workflows.
- `recv_notification`: receive the next raw server notification.
- `recv_event`: receive and decode the next notification as a `ServerEvent`.
- `shutdown`: close stdin, request child termination, and wait briefly for exit.
