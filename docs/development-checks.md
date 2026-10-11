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

## CI scope

`scripts/ci_scope.py` selects the Rust, desktop, TUI, and Python-kernel jobs for
pull requests. A TUI-only PR runs TUI tests without the Rust workspace matrix;
a desktop-only PR runs the desktop checks (including protocol generation).
Rust crates and embedded Python/skill assets select every component. Unknown
paths, build/CI configuration, and unavailable diffs also select every component.
Renames include both the removed and added paths. Pushes to the main branches
always run full CI. The separate runtime-bundle workflow remains unchanged.

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
