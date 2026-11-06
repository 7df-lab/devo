# Devo user guide

> Shell and terminal execution follow the configured sandbox profile; there is
> no global sandbox-disable switch. Python kernels are trusted user-privileged
> processes, not OS-isolated environments. See
> [the execution boundary](trusted-kernel-host-shell.md).
> Permission-picker UI flags and new-session permission defaults are separate
> settings; removing the global switch does not change them.


[Back to README](../README.md)

Detailed runtime, installation, and configuration guidance. For the short
introduction and quick start, see the README.

## Goals and managed background work

An active goal continues after a turn ends naturally. If that session still owns
managed background commands, a parked Python cell, or child-agent work, Devo
waits for their terminal events instead of starting repeated goal turns. The
session stays idle and accepts user input and controls while it waits.

Terminal output is retained before the wait is released and included as tool
context in the next model request. All outstanding work must settle; failure,
and cancellation release their own dependency so the parent can decide what to
do. A child waiting on an interactive approval/input request remains pending
until its turn resumes or ends. Queued user input takes priority over automatic
completion follow-ups and goal continuation. Pause, clear, replacement, and
session interruption invalidate the old automatic wake.

Only work managed by Devo is covered. Arbitrary detached OS processes and
client-owned terminals are not goal dependencies. The wait is runtime-only:
server restart does not restore or wait for old processes. Existing goal
recovery/resume rules still apply, and stale completion callbacks are ignored
after session teardown. Result context retains at most 32 completions with an
8,000-character tail per completion; full output remains in the existing task or
kernel result store where supported.

## IPython: a persistent workspace for recursive agents

In Devo's Recursive Agent mode, the model-facing `ipython` tool runs a
**persistent Python REPL**. It is a programmable control environment, not a
Jupyter notebook, and does not require the third-party IPython package.
Imports, variables, and intermediate results remain available across calls
within a session.

The regular function tool is `ipython`; file operations and shell commands run
through that workspace. Provider-hosted tools such as `web_search` appear
separately, so their list does not describe all the agent's capabilities.
On Windows, shell commands require Git for Windows. Devo detects Git Bash from
its installer registration or standard installation folders, including custom
drives. Set `DEVO_BASH_SHELL` to an absolute shell path for other installations.

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
2. Choose a working directory and start Devo (provider setup opens automatically):

   ```bash
   cd /path/to/your/workspace
   devo
   ```

3. Start or resume a terminal session:

   ```bash
   devo
   devo resume <session-id>
   ```

Prefer a graphical workspace? Install the [Desktop app](#desktop-app), then
connect a model provider in its setup flow.

While the TUI is working, **Enter** steers the current turn at its next model
request boundary. **Alt+Enter** explicitly queues a separate follow-up turn.
Steering does not interrupt a tool or model request already in progress;
press **Escape** to interrupt it.

Scroll the TUI transcript with the mouse wheel or **PageUp/PageDown**.
Press **Ctrl+End** to jump to the latest output and resume following it.
**Ctrl+Shift+Down** also works when your terminal forwards it; Windows Terminal
normally reserves that shortcut for its own scrolling.

## Features

| Interface | What it offers |
| --- | --- |
| **Desktop** | Visual setup, workspace conversations, session browsing, and model controls. |
| **TUI / CLI** | A terminal-native agent workflow, including session resume and command-line access. |
| **Shared runtime** | Persistent Python execution, model connections, permissions, MCP servers, skills, and child-agent sessions. |

## Models and providers

Devo supports configurable connections for
**OpenAI-compatible Chat Completions**, **OpenAI-compatible Responses**, and
**Anthropic Messages** APIs.
There are two model data sources: the live models.dev directory and your saved
configuration. No provider/model directory is bundled, and remote responses are
not saved as disk snapshots. A configured connection pins its endpoint, protocol,
and model IDs so remote updates cannot silently reroute existing requests.

Desktop onboarding, Settings, and the composer share this server-owned directory.
Use **Refresh catalog** to check models.dev for updates. In the TUI, `/model`
shows the current in-memory directory and checks in the background at startup
and while the picker is open. The default refresh interval is one hour.
Failed updates preserve the last successful directory within the same server
process and all user-configured models; they never switch your active model.

Provider **Discover** is an explicit, read-only request, including for Codex.
Results are temporary suggestions. Saving a discovered model pins only its ID and
route, not fetched prices or capabilities. Editing a catalog model saves only the
fields you change; untouched metadata continues to follow the live directory.
Opening a model picker does not discover or save models. Subscription sign-in
remains available even when the public directory is empty.

To change refresh behavior, add to `~/.devo/config.toml`
(Windows: `%USERPROFILE%\.devo\config.toml`):

```toml
[catalog]
refresh_interval_hours = 1
# offline = true
# source = "/path/to/models.dev-api.json"
```

`offline = true` prevents catalog and provider-discovery network requests.
An explicitly configured local source can still be refreshed. On a fresh offline
start there is no public directory: only your configured models are available.

Existing configuration files are preserved. Older sparse connections without a
pinned endpoint need an explicit reconnect or endpoint configuration. Old catalog
snapshot files are ignored, not deleted. The Native TUI also ignores the legacy
`models.json` directory; configure models through the server-backed provider settings.
This favors live metadata and stable
configured routes over first-run offline browsing.

Model requests go to the endpoint you configure. For local model traffic,
choose a compatible local endpoint.

### Private or custom backends

For example, a user-owned `custom-providers.json` can define a compatible private
backend and a model-specific route:

```json
{
  "provider": {
    "private": {
      "name": "Private backend",
      "base_url": "https://gateway.example/v1",
      "wire_api": "openai_chat_completions",
      "credential": "private_api_key",
      "models": {
        "review": {
          "base_url": "https://review.example/v1",
          "wire_api": "openai_responses",
          "request_model": "organization/review-v2"
        }
      }
    }
  }
}
```

Select `private/review`; its upstream request uses `organization/review-v2` at
the model-specific endpoint with Responses. Store the secret through provider
setup, not in this file; `credential` references its saved ID.

models.dev providers using unsupported SDK packages are omitted rather than
assigned a guessed protocol. OpenAI, Anthropic, and Google adapter defaults are
protocol behavior, not another model directory. Explicit connection saves the
resolved `base_url` so later catalog changes cannot silently reroute it.

### Errors during a conversation

A saved API key does not guarantee available credits. If an OpenAI-compatible
Responses stream reports `error` or `response.failed`, Devo ends that turn as a
failure and displays the provider's error in the conversation, even when no
assistant text arrived. Text already received remains visible.

Within these stream events, `insufficient_quota` and `credit_balance_exhausted`
are quota failures, not login failures. Devo does not automatically retry them
or invalidate the saved credential. Add credits or choose another configured
provider, then send a new message. The failed turn remains visible when the
session is resumed. Non-success HTTP responses retain their existing status-code
classification; this handling applies to failures inside an established SSE stream.

## Installation

### Desktop app

Download the package for your operating system and architecture from
[GitHub Releases](https://github.com/7df-lab/devo/releases/latest). Release
assets include macOS `.dmg`/`.zip`, Windows `.exe`, and Linux `.AppImage`/`.deb`/`.rpm`
packages. Check the asset name for the right architecture before installing.

Desktop packages include the Rust backend and a private Python interpreter with
the agent's Python dependencies. You do not need to install Python separately.

### Terminal TUI / CLI

**Linux or macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm 'https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1' | iex
```

The installer includes the Rust backend, compiled TUI, private Node.js 24 and
Python 3.13 runtimes, locked Python dependencies, and ripgrep. No system Node.js,
Python, npm, pip, Bun, or Rust installation is needed.
Windows bundles also include the sandbox setup and command-runner executables;
keep them beside `devo.exe`. The installer repairs installations missing these helpers.
On the first Python tool use, Windows requests UAC consent to provision the sandbox.
Approve it once; subsequent sessions reuse the setup. Canceling setup prevents that
sandboxed Python kernel from starting. To provision it explicitly, run
`devo sandbox-setup` and approve the Windows prompt.
The Windows online installer compares the verified app checksum before skipping
an existing version, so re-running it also installs a rebuilt release with the
same version number while reusing unchanged cached runtimes.

Online installers download the smaller app package and cache checksum-pinned
Node/Python runtimes separately. Upgrades reuse unchanged runtimes without
downloading or extracting them again. No package manager runs during installation.
On Windows, online app and runtime packs use `.zip` archives and are extracted
with the .NET ZIP reader; Unix packs remain `.tar.gz`. Older Windows indexes that
still reference `.tar.gz` packs remain installable.
The cache defaults to `%LOCALAPPDATA%\devo\runtimes` on Windows and
`${XDG_CACHE_HOME:-~/.cache}/devo/runtimes` on Unix. Set `DEVO_RUNTIME_CACHE` before
installing to choose another persistent directory. Keep this cache while the
installation uses it; the app's `runtime/*.path` files reference it. Re-running
the installer repairs missing runtimes. Old runtime packs remain available to
running terminals and older installations; they are not automatically deleted.

If an install is interrupted while downloading a private runtime, a stale
runtime-cache lock can remain. Devo does not delete stale locks automatically,
so it cannot remove a lock held by another installer. If the installer times
out, confirm that no other installer is running, remove the lock directory
named in the timeout message, then rerun the installer.

Linux terminal archives use names such as `devo-tui-v0.2.0-x86_64-linux-musl.tar.gz`
and `devo-tui-v0.2.0-aarch64-linux-musl.tar.gz`. Online app/runtime components
use the same platform suffix.

For portable/offline use, download the **complete** `devo-tui-` archive, extract
it together, and run its `devo` executable from any working directory. Copying
only `devo.exe` or `devo` is insufficient. Files named `devo-tui-app-` and
`devo-runtime-` are online installer components, not standalone portable bundles.

For a release mirror or CDN, set `DEVO_RELEASE_BASE_URL` to an HTTPS base URL
that serves `/<version>/<asset>` with the original asset names, installation
index and `SHA256SUMS.txt`. A pinned `VERSION=v0.2.0` also avoids GitHub version
lookups, so a reachable mirror can serve installs without GitHub access.
The default origin is GitHub Releases. Runtime
asset names contain their SHA-256; keep them immutable and use long CDN cache
lifetimes. The installer verifies the index and every downloaded archive before
installation. Configure the mirror before running the installer; no CDN service
or third-party credentials are required by Devo.

To serve only the shared Node/Python packs from a separate CDN, set
`DEVO_RUNTIME_BASE_URL=https://your-cdn.example/runtimes`. This serves
`/<runtime-asset-name>` directly, without a release-version directory, so
unchanged runtimes keep the same CDN URL across releases. The app and verified
installation index still come from `DEVO_RELEASE_BASE_URL` (or GitHub Releases).
Both mirror variables are optional; the normal install commands work as shown.

Terminal archives use the `devo-tui-` prefix, for example
`devo-tui-v0.2.0-x86_64-pc-windows-msvc.zip`. Graphical installers use the
`devo-desktop-` prefix. The installed terminal command remains `devo`.

Supported systems: 64-bit Windows (x64/ARM64), macOS (Intel/Apple Silicon), and
Linux (x64/ARM64 with glibc 2.28 or later). The Linux archive's `musl` suffix
describes the Rust backend; its bundled Node/Python runtimes require glibc.
Alpine/musl-only installations are not supported by the complete bundle.

To upgrade an existing CLI installation, run `devo upgrade`.
If you installed the original incomplete v0.2.0 archive, rerun the installer;
it detects the missing runtime files and repairs that same version.

### Offline installation and use

On a connected computer, download the complete CLI archive for the destination
OS/architecture and `SHA256SUMS.txt` from the same
[release](https://github.com/7df-lab/devo/releases). Verify its SHA-256,
then transfer both files to the offline computer and extract the archive.
All TUI and Python dependencies are included; first launch does not run npm,
pip, or download an interpreter.

Run the extracted executable directly, or use the installer included in the
extracted directory:

```bash
sh ./install.sh --offline
```

```powershell
.\install.ps1 -Offline
```

Installation itself does not require administrator privileges. Windows sandbox
provisioning separately requires UAC consent, as described above. Close running Devo instances before
upgrading on Windows. Installers stage the full application and restore the
previous files if replacement fails; provider credentials and conversations
remain in your Devo home directory.

An offline installation does not make cloud models available offline. Configure
a compatible local model endpoint with model weights already downloaded, or an
accessible LAN/private gateway. External MCP servers and optional packages need
their own offline preparation. To disable update and model-catalog refreshes,
add these settings to `~/.devo/config.toml` (or your `DEVO_HOME/config.toml`):

```toml
[updates]
enabled = false
check_on_startup = false

[catalog]
offline = true
refresh_on_startup = false
```

These settings control update/catalog traffic; model providers, tools, and MCP
servers follow their own network configuration. Run `devo doctor` to check the
private runtimes and Python dependencies before starting a session.

## Configuration

`devo` is the simplest way to connect a provider. In the Desktop app,
use the provider setup flow. Provider/model connections and API credentials are
stored separately: built-in provider connections use `providers.json`, custom
providers use `custom-providers.json`, the default model is selected in
`config.toml`, and user-scoped secrets use `auth.json`. Do not put API keys in
provider definitions or commit credential files.

Use `devo mcp` subcommands (`add`, `list`, `enable`, `disable`, `remove`)
to manage MCP servers. For the terminal interface, see the
[TUI guide](../apps/tui/README.md).
