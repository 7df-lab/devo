# Devo user guide

> **Temporarily disabled.** OS sandboxing and the permission picker are
> disabled repo-wide: every process runs with full user permissions and new
> sessions default to full-access. Revert by flipping
> `SANDBOX_ENFORCEMENT_TEMPORARILY_DISABLED` (Rust, `crates/sandbox/src/lib.rs`),
> `PERMISSIONS_COMMAND_ENABLED` (TUI), `PERMISSION_PICKER_ENABLED` (desktop),
> and the default-preset literals back to their asking defaults.


[Back to README](../README.md)

Detailed runtime, installation, and configuration guidance. For the short
introduction and quick start, see the README.

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
Its bundled catalog includes model definitions for providers such as DeepSeek,
Qwen, Kimi, GLM, and MiniMax. You can configure a compatible endpoint or private
gateway rather than relying on the bundled entries.

The built-in MiniMax endpoints offer `MiniMax-M3` and `MiniMax-M3.1-Flash`;
Flash is the TUI's MiniMax default. Both have a 1,000,000-token context window
and a documented 524,288-token output maximum. Devo exposes text and image
input; the API also accepts video, which Devo's input types do not yet represent.
Flash always uses adaptive thinking, with low/medium/high/xhigh/max effort
(default max). M3 supports adaptive thinking or disabled thinking; its Anthropic
API default is disabled and it ignores effort controls. MiniMax caching is
automatic; no manual cache-control markers or unpublished cache-write rate are
added. Cost totals omit cache writes when their price is unavailable.

MiniMax catalog prices are USD per million tokens. Flash uses $0.15 input,
$0.60 output and $0.015 cache read. M3's flat catalog estimate uses Standard
prices for inputs up to 512k: $0.30/$1.20/$0.06; above 512k the published rates
are $0.60/$2.40/$0.12. Priority pricing is 1.5 times the corresponding M3 rate.
The flat estimator does not model those M3 tiers, and Flash has no service-tier
parameter. See the [MiniMax API reference](https://platform.minimax.io/docs/api-reference/text-chat-anthropic.md)
and [pricing](https://platform.minimax.io/docs/pricing/overview.md).

Desktop onboarding, Settings, and the composer share the server's provider and
model catalog. Onboarding shows all available providers, supports searching by
provider or model, and refreshes remote model metadata. Use **Refresh catalog**
to check for updates; the saved catalog remains available offline. Catalog providers
and remotely fetched models cannot be deleted. You can disconnect a provider,
change model preferences, or remove models you added manually.

In the TUI, `/model` shows the saved catalog immediately and checks for updates
in the background at startup and while the picker is open. The public catalog
is refreshed at most once per hour by default; connected Codex accounts also
refresh their authenticated model directory. Search and selection stay in place.
Failed updates keep the last complete catalog, and never switch your active model.

To change the public catalog refresh interval or use an offline catalog, add to
`~/.devo/config.toml` (Windows: `%USERPROFILE%\.devo\config.toml`):

```toml
[catalog]
refresh_interval_hours = 1
# offline = true
# source = "/path/to/models.dev-api.json"
```

`offline = true` prevents catalog and provider-discovery network requests.
Local catalog files can still be refreshed, and the bundled catalog is always available.

Model requests go to the endpoint you configure. For local model traffic,
choose a compatible local endpoint.

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
The cache defaults to `%LOCALAPPDATA%\devo\runtimes` on Windows and
`${XDG_CACHE_HOME:-~/.cache}/devo/runtimes` on Unix. Set `DEVO_RUNTIME_CACHE` before
installing to choose another persistent directory. Keep this cache while the
installation uses it; the app's `runtime/*.path` files reference it. Re-running
the installer repairs missing runtimes. Old runtime packs remain available to
running terminals and older installations; they are not automatically deleted.

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
