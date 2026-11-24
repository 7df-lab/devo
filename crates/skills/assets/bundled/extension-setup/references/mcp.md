# Build and Install an MCP Server in Devo

## Resolve the server and transport

Use the provider's published entry point, package version, or MCP endpoint for an existing server. A local `stdio` server is a child process and needs the exact executable, arguments, working directory, and environment. A remote Streamable HTTP server needs the MCP URL and the provider's documented authentication. Select SSE only for a server that requires the older transport.

For a new server, use the project's language and a maintained MCP SDK. Start with the tools or resources the user asked for. Define typed input, validate it at the server boundary, and constrain the files or accounts the capability can access. Put a local server in a durable project or user directory with reproducible dependencies. Prefer `stdio` for a local integration and send logs to stderr so stdout carries only MCP messages. Use Streamable HTTP when a separately hosted endpoint is needed. Verify that the SDK and transport interoperate with the installed Devo version. [Official MCP server guide](https://modelcontextprotocol.io/docs/develop/build-server)

Before registering newly built code, run its tests or use an MCP client to initialize, list the expected capabilities, and exercise a representative read-only operation when one exists. Use the same launch command, arguments, working directory, and relevant environment as Devo. A successful subprocess launch alone does not verify the MCP handshake. [MCP Inspector](https://modelcontextprotocol.io/docs/tools/inspector)

## Persist the registration

Check the effective workspace `.devo/config.toml` and the user's MCP entries before choosing an ID. `devo mcp list` shows the user TOML config, and `devo mcp add` replaces the entire user record if the ID already exists. If the user has legacy TUI `mcpServers` in `<DEVO_HOME>/settings.json`, check for the same ID there too. A workspace override can take precedence over a user record with the same ID.

Use `devo mcp add` for straightforward user registrations. Run `devo mcp add --help` for the installed version's flags. These examples use non-secret values.

```text
devo mcp add local-notes --cwd "<server-dir>" -- "<absolute-executable>" "<server-entrypoint>"
devo mcp add --transport http remote-notes https://notes.example/mcp
devo mcp list
```

Use `<workspace>/.devo/config.toml` for workspace registration. For features the CLI cannot express, such as environment variable references, edit just the relevant `[mcp_servers.<id>]` tables in the selected config. The user config is `<DEVO_HOME>/config.toml`. If the CLI is unavailable, use the same targeted TOML edit for a user registration. Preserve unrelated settings and validate the resulting TOML.

```toml
[mcp_servers.local-notes]
command = "<absolute-executable>"
args = ["<server-entrypoint>"]
cwd = "<server-dir>"
env_vars = ["NOTES_TOKEN"]

[mcp_servers.remote-notes]
url = "https://notes.example/mcp"
type = "streamable_http"

[mcp_servers.remote-notes.env_http_headers]
Authorization = "NOTES_AUTHORIZATION"
```

Use only the example that matches the target, with the real paths and required environment variable names. `env_vars` forwards the named variables. `env_http_headers` maps a header to an environment variable carrying the full header value, including `Bearer ` when required. Set these in the environment of the Devo server process. For OAuth, use the client's existing sign-in flow when supported. CLI `--env`, `--header`, and `--bearer-token` accept literal values that can be persisted in plaintext, so use environment references for secrets.

## Verify activation

1. Confirm the saved ID and transport in the CLI list for user config or in the workspace config. If the matching server is disabled and enablement was requested, use the existing enable action or adjust its `enabled` field at the owning scope.
2. Check live discovery in the target workspace. With Devo's Python host MCP helpers, use `await mcp.list_tools("server-id")`. For user servers, `/mcps` or Native `mcp/list` and `mcp/tools` (`{"name":"server-id"}`) show the runtime state and tool names. Check the expected capabilities. If the server provides only resources or prompts, verify those with an MCP client that supports them. When safe and relevant, test a read-only call through Devo.
3. Use live discovery before reporting success. If the running backend still shows the previous configuration after a file edit, restart it before verifying the connection. If discovery fails after the configuration is loaded, check the server's launch paths, stderr, and authentication without exposing secrets.
