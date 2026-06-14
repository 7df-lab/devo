# devo-cli

`devo-cli` is the binary entry point for Devo. It wires together the core,
provider, TUI, server, safety, MCP, and task crates behind the `devo` command.

Running `devo` with no subcommand starts the interactive agent UI. The crate
also owns the top-level command dispatch for onboarding, session resume,
single-prompt execution, diagnostics, upgrades, and the hidden runtime server
entry point.

The process is started through `devo_arg0::run_as`, which lets the same binary
serve both the normal CLI and alias-based helper entry points such as
`devo-server`.

## Usage

```sh
devo                         # start the interactive agent UI
devo onboard                 # configure a model provider
devo resume <session-id>     # resume a saved session
devo prompt "Explain this"   # run one non-interactive prompt
devo doctor                  # check configuration and connectivity
devo upgrade                 # install the latest released version
```
