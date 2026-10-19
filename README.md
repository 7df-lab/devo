<div align="center">

<img src="./.github/assets/devo-readme-brand.svg" alt="Devo" width="360" />

</div>

# Devo — Open-source recursive agent

Devo tackles multi-step work with a **persistent Python workspace**, **child
agents**, and **your choice of model provider**. Use the Desktop app or terminal
TUI/CLI, powered by a local Rust runtime.

[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)

[English](./README.md) · [简体中文](./README.zh-Hans.md) · [繁體中文](./README.zh-Hant.md) · [日本語](./README.ja.md) · [Русский](./README.ru.md)

## Devo vs. Claude Code and Codex

| Agent | Main emphasis | Models |
| --- | --- | --- |
| **Devo** | Programmable, recursive execution for coding and other multi-step tasks. | Compatible cloud providers, local endpoints, and private gateways. |
| [Claude Code](https://code.claude.com/docs/en/overview) | Claude-centered development across terminal, IDE, desktop, and web. | Claude through Anthropic or supported cloud providers. |
| [Codex](https://learn.chatgpt.com/docs/codex/cli) | OpenAI's coding agent across terminal, IDE, app, and cloud. | OpenAI models by default; the CLI also supports [custom and local providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers). |

Devo's `ipython` tool keeps variables and intermediate results across calls.
The agent can filter large outputs in Python, reuse earlier work, and delegate
independent subtasks to child agents with their own sessions and Python state.
MCP, skills, permissions, and resumable sessions round out the workflow.

## Quick start

**Desktop:** Download the package for your OS and architecture from
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest), then connect
a model provider in the setup flow.

**Terminal — Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**Terminal — Windows PowerShell:**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

Start in your workspace; first launch opens provider setup:

```bash
cd /path/to/your/workspace
devo
```

Resume with `devo resume <session-id>`; upgrade with `devo upgrade`.
Terminal bundles include Node.js and Python. On Windows, install
[Git for Windows](https://gitforwindows.org/) for shell commands and approve
the one-time sandbox setup prompt.

> **Temporarily disabled.** OS sandboxing and the permission picker are
> disabled repo-wide: every process runs with full user permissions and new
> sessions default to full-access. Revert by flipping
> `SANDBOX_ENFORCEMENT_TEMPORARILY_DISABLED` (Rust, `crates/sandbox/src/lib.rs`),
> `PERMISSIONS_COMMAND_ENABLED` (TUI), `PERMISSION_PICKER_ENABLED` (desktop),
> and the default-preset literals back to their asking defaults.

## Configuration

Connect endpoints using **OpenAI-compatible Chat Completions**, **OpenAI-compatible
Responses**, or **Anthropic Messages** APIs. The bundled catalog includes
DeepSeek, Qwen, Kimi, GLM, and MiniMax. Use `/model` to switch models or
`devo onboard` to revisit provider setup.

For manual configuration, credentials, MCP, offline installation, and release
mirrors, see the [user guide](./docs/usage.md).

## Docs and contributing

- [User guide](./docs/usage.md) — installation, providers, configuration, and offline use.
- [TUI guide](./apps/tui/README.md) — terminal controls and commands.
- [Contributing](./CONTRIBUTING.md) — issues, documentation, and focused fixes welcome.

Devo is **pre-1.0**; APIs and configuration may change. [MIT License](./LICENSE).
