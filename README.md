<div align="center">

<img src="./.github/assets/devo-readme-logo.png" alt="Devo" width="220" />

</div>

<div align="center">

**A lightweight, model-neutral coding agent that runs as a single binary. Fast, token-efficient, and highly customizable.**

[![Stars](https://img.shields.io/github/stars/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/stargazers)
[![Language](https://img.shields.io/badge/language-Rust-E57324?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](./LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)](https://github.com/7df-lab/devo/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/7df-lab/devo/ci.yml?branch=main&style=flat-square)](https://github.com/7df-lab/devo/actions)
[![Release](https://img.shields.io/github/v/release/7df-lab/devo?style=flat-square)](https://github.com/7df-lab/devo/releases)
[![AUR version](https://img.shields.io/aur/version/devo-bin?color=%231793d1&label=AUR&logo=arch-linux&logoColor=%23fff&style=flat-square)](https://aur.archlinux.org/packages/devo-bin/)

[English](./README.md) | [简体中文](./README.zh-Hans.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [Русский](./README.ru.md)

[Features](#features) · [Tested Models](#tested-models) · [Tested Platforms](#tested-platforms) · [Install](#installation) · [Quick Start](#quick-start) · [Docs](#docs)

</div>

---

## Features

- **Built-in semantic code search** - Runs a local CPU code-embedding model and
  combines dense retrieval with BM25 keyword matching, reducing code-search
  context compared with grep/find-only agent.
- **Bring your own model provider** - Use provider/model bindings for
  OpenAI-compatible, Anthropic-compatible, DeepSeek, Qwen, Kimi, GLM, MiniMax,
  Xiaomi MiMo, OpenRouter, or local endpoints.
- **MCP support** - Connect external tools and context through
  [Model Context Protocol](https://modelcontextprotocol.io/) servers.
- **Skill support** - Package repeatable workflows, instructions, scripts, and
  references as reusable [Agent Skills](https://agentskills.io/).
- **Long-running task support** - Let Devo manage context automatically across
  multi-turn work instead of losing the thread as tasks grow.
- **Multi-agent support** - Split work across specialized agents while keeping
  coordination visible in the session.
- **Plan Mode** - Break larger tasks into clear multi-step plans before
  implementation starts.
- **Parallel tool calls** - Run multiple independent tools at parallel so
  models spend less time waiting and more time making progress.
- **Permissioned tool execution** - Review sensitive tool calls before they
  touch your workspace.
- **Auditable sessions** - Keep model output, tool calls, approvals, token
  usage, and session history inspectable and resumable.
- **Cost and context visibility** - Show input/output tokens, cached tokens, and
  context-window usage where providers expose them.
- **Lightweight Rust runtime** - Built in Rust with low memory overhead and a
  compact local runtime.

## Tested Models

<p>
  <img alt="DeepSeek v4 Flash / Pro" src="https://img.shields.io/badge/DeepSeek-v4%20Flash%20%2F%20Pro-4D6BFE?style=flat-square&logo=deepseek&logoColor=white" />
  <img alt="Qwen3 Coder Next" src="https://img.shields.io/badge/Qwen3-Coder%20Next-615CED?style=flat-square&logo=qwen&logoColor=white" />
  <img alt="Kimi K2.5" src="https://img.shields.io/badge/Kimi-K2.5-111111?style=flat-square&logo=moonshotai&logoColor=white" />
  <img alt="MiniMax M2.7" src="https://img.shields.io/badge/MiniMax-M2.7-0B5FFF?style=flat-square&logo=minimax&logoColor=white" />
  <img alt="GLM 5.1" src="https://img.shields.io/badge/GLM-5.1-7856FF?style=flat-square" />
</p>

Devo's built-in model catalog includes tested model definitions for Qwen, Kimi,
MiniMax, GLM, and DeepSeek. Provider endpoints remain configurable through
provider/model bindings.

## Tested Platforms

<p>
  <img alt="macOS tested" src="https://img.shields.io/badge/macOS-tested-000000?style=flat-square&logo=apple&logoColor=white" />
  <img alt="Linux tested" src="https://img.shields.io/badge/Linux-tested-FCC624?style=flat-square&logo=linux&logoColor=000000" />
  <img alt="Windows tested" src="https://img.shields.io/badge/Windows-tested-0078D4?style=flat-square&logo=windows&logoColor=white" />
</p>

Devo has been tested on macOS, Linux, Windows, and Kylin OS.

### For Chinese Enterprise Users

<p>
  <img alt="Kylin OS tested" src="https://img.shields.io/badge/Kylin%20OS-tested-1E88E5?style=flat-square" />
  <img alt="HarmonyOS support on the road" src="https://img.shields.io/badge/HarmonyOS-on%20the%20road-111111?style=flat-square&logo=harmonyos&logoColor=white" />
</p>

Kylin OS coverage is called out because domestic operating systems are often
part of real deployment requirements in Chinese enterprise environments.
HarmonyOS support is on the roadmap; contributors with HarmonyOS devices are
welcome to build, test, and publish releases for that platform.

## Screenshots

<p align="center">
  <img width="100%" alt="Devo running in a terminal" src="./.github/assets/devo-readme-screenshot.png" />
</p>

## Installation

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

### Windows

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

The online installer places `devo` under the Devo home directory, installs the
`rg` sidecar used for fast repository search, and can preinstall the local
Hugging Face model used by `code_search`.

Preinstall the local `code_search` model:

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --install-code-search-model
```

Windows:

```powershell
$env:DEVO_INSTALL_CODE_SEARCH_MODEL = "1"; irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

Upgrade an existing installation to the latest release:

```bash
devo upgrade
```

The upgrade command runs the same platform installer, and the installer prints
the version transition, for example `Version: v0.1.12 -> v0.1.15`.

<details>
<summary>Offline Installation</summary>

Many enterprise and intranet environments do not have internet access. Devo's
installers support an offline mode that reads all required assets from the same
directory as the installer script and does not contact the network.

On a machine with internet access:

1. Download the installer script: `install.sh` for Linux/macOS or `install.ps1`
   for Windows.
2. Download the latest Devo release asset for the target CPU and OS, for example
   `x86_64` vs `aarch64`/`arm64`.
3. Download the Hugging Face `minishlab/potion-code-16M` model files used by
   local semantic `code_search`: `config.json`, `model.safetensors`, and
   `tokenizer.json`.
4. Download the matching `ripgrep` release asset for the target CPU and OS.

Place these files next to the installer script. The model files can either sit
next to the installer directly or under a `minishlab--potion-code-16M/`
subdirectory.

Linux / macOS:

```bash
sh ./install.sh --offline
```

Windows:

```powershell
.\install.ps1 -Offline
```

Offline mode installs the model into
`<DEVO_HOME>/local-models/minishlab--potion-code-16M`, which is the directory
used by the runtime code-search provider. When `DEVO_HOME` is not set, this is
`~/.devo/local-models/minishlab--potion-code-16M`.

</details>

## Quick Start

Configure a provider, open a repository, and start the TUI:

```bash
cd /path/to/your/repo
devo onboard
```

Useful commands:

```bash
devo                         # start the interactive TUI in the current repo
devo resume <session-id>
```

## Configuration

`devo onboard` is the recommended setup path. For manual configuration, Devo
merges settings in this order:

1. Built-in defaults
2. `DEVO_HOME/config.toml` - user-level config, defaulting to `~/.devo/config.toml`
   on macOS/Linux and `C:\Users\yourname\.devo\config.toml` on Windows
3. `<workspace>/.devo/config.toml` - project-level config
4. CLI flags

Credentials live separately in `DEVO_HOME/auth.json`; `config.toml` should refer
to credential ids instead of storing API keys directly.

Minimal shape:

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
model_name = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

The important separation is:

- `model_slug` selects Devo's local model metadata from `models.json`.
- `provider` selects the configured connection record.
- `model_name` is the provider-specific model string sent on the wire.
- `invocation_method` selects the provider protocol, such as
  [`openai_chat_completions`](https://developers.openai.com/api/reference/chat-completions/overview),
  [`openai_responses`](https://developers.openai.com/api/reference/responses/overview),
  or [`anthropic_messages`](https://platform.claude.com/docs/en/api/messages).

### Custom Models

If the model you want to use is not in the built-in list, add it to
`models.json`, then bind it through `config.toml`.

User-level model catalog:

- macOS/Linux: `~/.devo/models.json`
- Windows: `C:\Users\yourname\.devo\models.json`

Project-level overrides can also be placed at `<workspace>/.devo/models.json`.
Catalog precedence is `<workspace>/.devo/models.json`, then
`<DEVO_HOME>/models.json`, then the built-in catalog.
In `models.json`, `provider` is the default wire API metadata for the model; the
actual endpoint is still selected by the `provider` field in `config.toml`.

Example `models.json` entry:

```json
[
  {
    "slug": "my-coding-model",
    "display_name": "My Coding Model",
    "channel": "Custom",
    "provider": "openai_chat_completions",
    "description": "Custom OpenAI-compatible coding model.",
    "reasoning_capability": "unsupported",
    "context_window": 200000,
    "effective_context_window_percent": 95,
    "max_tokens": 4096,
    "input_modalities": ["text"],
    "base_instructions": "You are Devo, a coding agent. Help the user edit and understand code."
  }
]
```

Then reference that `slug` from a model binding:

```toml
[model_bindings.my-coding-model-example]
enabled = true
model_slug = "my-coding-model"
provider = "my.provider"
model_name = "provider-specific-model-name"
display_name = "My Coding Model"
invocation_method = "openai_chat_completions"
```

## FAQ

### What is the project status?

Devo is pre-1.0 and actively developed. It is ready for local evaluation,
experiments, and contributor use; public APIs and configuration may still
change.

### What models are supported?

Built-in model metadata currently covers Qwen, Kimi, MiniMax, GLM, and DeepSeek
families. Any model endpoint that supports OpenAI-compatible Chat Completions,
OpenAI-compatible Responses, or the Anthropic Messages API can be connected through
provider/model bindings.

## Contributing

Contributions are welcome while the project is still early:

- Architecture feedback on the client/server runtime, provider layer, safety
  model, and TUI.
- Documentation and translations.
- Provider, model, and wire API coverage.
- Focused fixes with validation commands and regression tests.

Open an issue or pull request to discuss changes.

## Star History

<a href="https://www.star-history.com/?repos=7df-lab%2Fdevo&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=7df-lab/devo&type=date&legend=top-left" />
 </picture>
</a>

## License

This project is licensed under the [MIT License](./LICENSE).

---

**If you find Devo useful, please consider giving it a star.**
