# Fast development checks

Use the existing Cargo workspace and check the component being edited first.
The Node TUI loads TypeScript source directly; UI edits do not require rebuilding
the Rust server. Keep one built server and use `DEVO_SERVER_BIN` when launching
`npm start` from `apps/tui`.

```sh
# A targeted TUI regression; no Rust compilation.
cd apps/tui
node --import tsx --test src/native-to-pi/tools.test.ts
npm test

# From the repository root: type-check one Rust crate without linking binaries.
cargo check --locked -p devo-harness

# Compile/run only the relevant crate's unit tests during iteration.
cargo test --locked -p devo-harness --lib

# Select an integration-test executable before applying a test-name filter.
cargo test --locked -p devo-server --test goal_continuation

# Build the CLI only when the Rust runtime changes.
cargo build --locked -p devo-cli --bin devo
```

On Windows, also build the sandbox helper binaries when they change:
`cargo build --locked -p devo-windows-sandbox --bins`.
Do not overwrite a running Windows executable; stage a runtime copy as the
desktop development launcher does, or close only your own test session first.

Avoid `cargo test --workspace` for every edit: a test-name filter alone still
builds the selected packages' test executables. Use `-p`, `--lib`, or `--test`
to bound that compilation. Broaden tests when shared contracts or dependencies
change. Full workspace validation remains necessary before merging Rust changes.

Keep `RUSTFLAGS`, profiles, and the target directory stable to reuse Cargo's
incremental artifacts. An isolated worktree can reuse a dedicated QA target
directory through `CARGO_TARGET_DIR`; avoid concurrent builds against the same
target directory because Cargo serializes them. TypeScript and Python changes
can be tested without waiting for that Rust build lock.

The server's stdio test helpers resolve CLI and MCP fixture binaries beside
the running test's `deps` directory. Their on-demand Cargo builds use that
same profile (`debug` maps to Cargo's `dev` name), so `dev-fast` tests cannot
silently select an older `target/debug/devo`. Cargo checks fixture freshness
once per test process; subsequent child launches reuse the checked binary.
The build pins the canonical target directory from Cargo's compiled test
metadata and preserves an explicit target-triple subdirectory. Relative
`CARGO_TARGET_DIR` values therefore retain the same cache when the fixture
build changes working directory. Missing or unexpected output fails visibly. An explicit `CARGO_BIN_EXE_devo`
fixture override remains supported. These helpers change test execution only.

## Optional faster Rust iteration

Use `dev-fast` when filename/line backtraces are enough for the edit:

```sh
cargo build --locked --profile dev-fast -p devo-cli --bin devo
cargo test --locked --profile dev-fast -p devo-server --lib compact
```

This profile inherits `dev` and changes only debug information to
`line-tables-only`. Incremental compilation, debug assertions, overflow checks
and optimization settings still come from `dev`. It keeps filename/line
backtraces but omits variable and parameter debugger information. Use the
normal `dev` profile when inspecting those values in a debugger. Default builds,
tests and release packaging keep their existing profiles.

Artifacts are under `target/dev-fast` (or the selected `CARGO_TARGET_DIR`).
Point `DEVO_SERVER_BIN` at its `devo` binary when running the development TUI.
On Windows, build the helper binaries in that profile before using the sandbox:

```sh
cargo build --locked --profile dev-fast -p devo-windows-sandbox --bins
```

The first build warms a separate profile cache; keep using the same profile
and target directory during an edit cycle. Switching profiles is not a way to
avoid that one-time cost.

Two small server log-string edits on one Windows machine, with both profiles
warm and the order reversed for the second edit, gave:

| Edit | Normal dev | dev-fast |
| --- | ---: | ---: |
| A (dev first) | 25.53s | 12.03s |
| B (dev-fast first) | 21.90s | 10.50s |

These are CLI rebuild wall times for a small source edit, not clean-build,
CI or arbitrary-change guarantees. Record `--timings` for the component being
changed before making broader compiler, crate-boundary or build-system changes.
Cargo documents the [debug information tradeoff](https://doc.rust-lang.org/cargo/reference/profiles.html#debug).

## CI scope

`scripts/ci_scope.py` selects the Rust, desktop, TUI, and Python-kernel jobs for
pull requests. A TUI-only PR runs TUI tests without the Rust workspace matrix;
a desktop-only PR runs the desktop checks (including protocol generation).
Rust crates and embedded Python/skill assets select every component. Unknown
paths, build/CI configuration, and unavailable diffs also select every component.
Renames include both the removed and added paths. Pushes to the main branches
always run full CI. The separate runtime-bundle workflow remains unchanged.

## TUI tmux smoke in CI

Two jobs drive the product TTY under tmux with the scenario runner
(`apps/tui/scripts/run-tmux-smoke.js`; scenarios described in the repository
AGENTS.md). Both build the debug CLI and boot the real TUI against a
per-run temporary Devo home; assertions read the Native protocol trace
(turn outcomes, RPC success), never exact model reply text.

- **`tmux` (secret-free, every qualifying PR):** boots the TUI (T1), runs the
  `/goal` and `/heartbeat` slash flows, and completes a real turn (T2) against
  a local mock provider (`apps/tui/scripts/smoke-mock-provider.mjs`, an
  OpenAI-completions SSE stub) seeded via `providers.json` +
  `auth.json` in `DEVO_HOME`. Fork PRs run this job because it needs no
  credentials.
- **`tmux-live` (repo-owned runs only):** completes real turns on all three
  model wire APIs, on `ubuntu-latest` and `macos-latest` (macOS exercises the
  seatbelt fence and the `python3` interpreter fallback):
  - **Chat Completions** — DeepSeek at `https://api.deepseek.com`
    (`openai_chat_completions`), using the `DEEPSEEK_API_KEY` repository
    secret; this configuration also runs the interrupt scenario (T3).
  - **Anthropic Messages** — DeepSeek's Anthropic-compatible endpoint
    `https://api.deepseek.com/anthropic` (`anthropic_messages`), same secret.
  - **Responses** — OpenAI at `https://api.openai.com/v1`
    (`openai_responses`, `gpt-5-mini`), using the optional
    `OPENAI_API_KEY` repository secret; the step is skipped when that secret
    is absent.
  The job is skipped when `DEEPSEEK_API_KEY` is absent — including fork PRs,
  where secrets are unavailable. Add secrets under *Settings → Secrets and
  variables → Actions*; spare low-quota keys are sufficient because each run
  issues a handful of short prompts.

The mock provider never validates credentials and speaks all three wires
(`--wire openai|anthropic|responses`), so the same seeded-home shape works
for local dry runs:

```sh
node apps/tui/scripts/smoke-mock-provider.mjs 8801 --wire anthropic &
home=$(mktemp -d)
printf '%s\n' '{"model": "deepseek/deepseek-v4-flash", "provider": {"deepseek": {"credential": "deepseek", "enabled": true, "base_url": "http://127.0.0.1:8801", "wire_api": "anthropic_messages"}}}' > "$home/providers.json"
printf '%s\n' '{"deepseek": {"type": "api_key", "key": "ci-mock-key"}}' > "$home/auth.json"
DEVO_HOME="$home" node apps/tui/scripts/run-tmux-smoke.js T2
```

Preview a committed change's scope without building:

```sh
python scripts/ci_scope.py --base HEAD~1 --head HEAD
python -m unittest discover -s scripts -p test_ci_scope.py -v
```

This change reduces unrelated builds and tests; it does not claim a measured
reduction in rustc's per-crate compilation time. Before adopting another build
system or changing debug/linker settings, profile representative edits:

```sh
cargo build --timings -p devo-cli --bin devo
```

Cargo documents [package and target selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
and [build timing reports](https://doc.rust-lang.org/cargo/reference/timings.html).
A new build system is not required for component selection and would introduce
a second dependency graph to maintain.
