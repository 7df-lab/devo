<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo Recursive Agent logo" width="360" />

</div>

# Devo — Open-source Recursive Agent for multi-step work

Devo takes a **recursive, programmable approach** to complex tasks. Its agent
can keep working state in Python, inspect results step by step, and delegate
independent work to child agents. Software development is one use case, not its
limit. Use Devo in the **Desktop app** or **terminal TUI/CLI**, with a local Rust
runtime and your choice of compatible model provider or private gateway.

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

[IPython workspace](#ipython-a-persistent-workspace-for-recursive-agents) · [Quick start](#quick-start) · [Features](#features) · [Models and providers](#models-and-providers) · [Installation](#installation) · [Configuration](#configuration)

## IPython: a persistent workspace for recursive agents

In Devo's Recursive Agent mode, the model-facing `ipython` tool runs a
**persistent Python REPL**. It is a programmable control environment, not a
Jupyter notebook, and does not require the third-party IPython package.
Imports, variables, and intermediate results remain available across calls
within a session.

That changes how it works compared with one-off commands or a fixed menu of
tools:

- **Inspect selectively:** Use Python to search, parse, and filter files or
  tool output, then bring only the relevant results into the conversation.
- **Build on earlier steps:** Reuse parsed data and small helper functions in
  later calls instead of starting each tool invocation from a blank state.
- **Coordinate real tools:** Use `bash()` to run project commands in their own
  environments, and top-level `await` to manage asynchronous work.
- **Delegate focused work:** When available, spawn child agents for independent
  subtasks. Each child has its own session and Python state, reports findings
  to the parent, and does not return an answer from the spawn call itself.

The workspace remains subject to Devo's permission and sandbox rules. Recursion
does not give a child agent broader access than its approved environment.

## Quick start

1. [Install Devo](#installation) for your platform.
2. Choose a working directory and run onboarding:

   ```bash
   cd /path/to/your/workspace
   devo onboard
   ```

3. Start or resume a terminal session:

   ```bash
   devo
   devo resume <session-id>
   ```

Prefer a graphical workspace? Install the [Desktop app](#desktop-app), then
connect a model provider in its setup flow.

## Features

| Interface | What it offers |
| --- | --- |
| **Desktop** | Visual setup, workspace conversations, session browsing, and model controls. |
| **TUI / CLI** | A terminal-native agent workflow, including session resume and command-line access. |
| **Shared runtime** | Persistent Python execution, model connections, permissions, MCP servers, skills, and child-agent sessions. |

- **Work recursively:** Keep the parent focused on the overall task while child
  agents investigate independent parts and report back.
- **Bring your own model:** Select a provider and model without tying the agent
  to a single hosted model service. Local endpoints are supported when they
  implement a compatible API.
- **Plan and execute:** Review a plan before implementation, and approve
  sensitive tool actions when permissions require it.
- **Extend the agent:** Connect [Model Context Protocol](https://modelcontextprotocol.io/)
  (MCP) servers and package repeatable workflows as [Agent Skills](https://agentskills.io/).
- **Continue your work:** Inspect session history, resume previous sessions,
  and coordinate child agents on larger tasks.
- **Optional local code search:** The `code_search` MCP combines embeddings and
  keyword search. Its binary and model are not installed by default.

## Models and providers

Devo supports configurable connections for
**OpenAI-compatible Chat Completions**, **OpenAI-compatible Responses**, and
**Anthropic Messages** APIs.
Its bundled catalog includes model definitions for providers such as DeepSeek,
Qwen, Kimi, GLM, and MiniMax. You can configure a compatible endpoint or private
gateway rather than relying on the bundled entries.

Model requests go to the endpoint you configure. For local model traffic,
choose a compatible local endpoint.

## Installation

### Desktop app

Download the package for your operating system and architecture from
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest). Release
assets include macOS `.dmg`/`.zip`, Windows `.exe`, and Linux `.AppImage`/`.deb`/`.rpm`
packages. Check the asset name for the right architecture before installing.

### Terminal TUI / CLI

**Linux or macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

Review an installer before running it if your environment requires it.
The installer sets up the `devo` command and its `rg` search sidecar;
`code_search` is an optional extra.

<details>
<summary>Install the optional code-search MCP and local model</summary>

On Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --with-code-search
```

On Windows (PowerShell):

```powershell
$env:DEVO_INSTALL_CODE_SEARCH = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

After installation, enable the MCP with `devo mcp enable code_search` or
TUI `/mcps`.

</details>

To upgrade an existing CLI installation, run `devo upgrade`.

## Configuration

`devo onboard` is the simplest way to connect a provider. In the Desktop app,
use the provider setup flow. Provider/model connections and API credentials are
stored separately: built-in provider connections use `providers.json`, custom
providers use `custom-providers.json`, the default model is selected in
`config.toml`, and user-scoped secrets use `auth.json`. Do not put API keys in
provider definitions or commit credential files.

Use `devo mcp` subcommands (`add`, `list`, `enable`, `disable`, `remove`)
to manage MCP servers. For the terminal interface, see the
[TUI guide](./apps/tui/README.md).

## FAQ

### Can I use a local model or private gateway?

Yes, if its endpoint implements a supported API. Set the connection to that
endpoint and choose its provider/model entry. Model requests go to the
endpoint you configure.

### Is Devo ready for production deployments?

Devo is pre-1.0 and under active development. Evaluate it with your own model,
permissions, and deployment requirements; APIs and configuration may change.

### Can I switch between Desktop and the terminal?

Both interfaces use the Devo runtime. Choose Desktop for graphical setup and
session browsing, or the TUI/CLI for terminal-first work.

## Contributing

Issues, documentation updates, and focused fixes are welcome. Read
[CONTRIBUTING.md](./CONTRIBUTING.md) before opening a pull request.

## License

Devo is licensed under the [MIT License](./LICENSE).
