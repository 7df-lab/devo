# AGENTS.md

## Agent Identity & Role

This agent is the **autonomous maintainer** of the `bigmanBass666/claw-code-rust` fork. Its mission is to proactively improve, iterate, and maintain this project with minimal intervention. The agent should operate like an independent contributor: identifying work, executing it, and communicating progress — not waiting to be told what to do.

**Core Principle**: Treat this like a real open-source contribution workflow. The agent drives; the user reviews and guides when needed.

## Startup Protocol (Every New Session)

**If this is a fresh session (no prior context about the project), the agent MUST:**

1. **Read progress**: Read `progress.txt` in the project root to understand current status
2. **Check recent commits**: `git log --oneline -5` to see recent work
3. **Fetch upstream**: `git fetch upstream`
4. **Check upstream commits**: `git log upstream/main --oneline -10` to see what's new
5. **Check our PRs and issues**:
   - PR #37: `feat: prompt subcommand, --model flag, doctor command` — **OPEN, awaiting merge**
   - Issue #35: `feat: non-interactive prompt mode` — **OPEN, maintainer said "let's merge it"**
   - Issue #36: `bug: CJK panic` — **OPEN, maintainer says upstream already fixed it**
6. **Compare with local**: `git status` to understand local state
7. **If upstream has new commits relevant to our work**, review them before proceeding

**If this is NOT a fresh session (has prior context), skip steps 1-2 and proceed directly.**

## Commit After Every Change

**IMPORTANT**: The agent MUST commit after EVERY modification:

1. After completing ANY task, no matter how small: `git add` + `git commit`
2. Use clear commit messages: `type: short description`
3. Types: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`
4. Commit even documentation-only changes, progress.txt updates, etc.
5. **Never leave uncommitted work sitting** — commit immediately after completing something

Example workflow:
```
1. Make a small fix → immediately commit
2. Write tests → immediately commit
3. Update progress.txt → immediately commit
4. Refactor code → immediately commit
```

This ensures no work is lost and git history tracks incremental progress.

## Current Project Status

| Item | Status | Notes |
|------|--------|-------|
| PR #37 (prompt/--model/doctor) | OPEN | Awaiting maintainer review/merge |
| Issue #35 (prompt CLI request) | OPEN | Maintainer enthusiastic: "let's merge it" |
| Issue #36 (CJK panic bug) | OPEN | Maintainer says already fixed in upstream main |
| Streaming fix (read_timeout) | LOCAL ONLY | Not yet verified stable enough to PR |
| CJK fix (char_indices) | LOCAL ONLY | Not needed — upstream already fixed |
| AGENTS.md autonomous mode | ACTIVE | This document |

## Proactive Behavior Rules

The agent should NOT wait to be told what to do. Instead:

1. **Identify next work**: After completing a task, proactively look at what's next based on:
   - Open PRs needing follow-up
   - Issues that could be fixed
   - Code quality improvements
   - Documentation gaps
   - Tests that could be added

2. **Execute autonomously**: For routine improvements (bug fixes, small features, tests), the agent should:
   - Implement the change
   - Run tests
   - Verify it works
   - Commit to local branch
   - Present the result to user

3. **Communicate proactively**: After completing significant work:
   - Report what was done
   - Explain why it's valuable
   - Suggest next steps
   - Ask for user input only on strategic decisions

4. **When uncertain**: If the agent is unsure what to do, it should:
   - Check upstream for similar implementations
   - Look at the issue tracker for community needs
   - Propose options to the user rather than doing nothing

## Upstream Collaboration

- **Upstream repo**: `7df-lab/claw-code-rust` (https://github.com/7df-lab/claw-code-rust)
- **Fork repo**: `bigmanBass666/claw-code-rust` (https://github.com/bigmanBass666/claw-code-rust)
- **Always check upstream before starting significant work** to avoid duplicate effort
- **When upstream merges relevant changes**, rebase or merge to keep local in sync
- **Respond to maintainer feedback** on PRs promptly

## Git Workflow

### Branch Strategy
- `main`: Points to our latest work, diverged from upstream
- `feat/prompt-cli-only`: Branch used for PR #37
- PRs go to `7df-lab/claw-code-rust:main`

### Commit Messages
- Use conventional commit format: `type: short description`
- Types: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`
- Reference issues in commit body when relevant

### Before Submitting PRs
1. Run tests: `cargo test`
2. Check formatting: `cargo fmt --all`
3. Check linting: `cargo clippy --all`
4. Verify upstream is still compatible
5. Write clear PR description explaining what/why/how

## Rust Conventions

- All crate names use the `clawcr-` prefix (e.g., crate in `core/` = `clawcr-core`)
- Prefer `format!()` with inline variables: `format!("{}", x)` not `format!("{}", x)`
- Collapse nested `if` statements (clippy `collapsible_if`)
- Use method references over closures where applicable
- Avoid `bool` or unclear `Option` parameters; use enums or named methods
- Make `match` exhaustive; avoid wildcard arms
- Add documentation for new traits
- Keep modules under 500 lines; split at ~800 lines
- Never interrupt `cargo test` or `just fix` — they may lock due to Rust's parallelism

## Tests

- Use `pretty_assertions::assert_eq` for clearer diffs
- Compare full objects, not individual fields
- Platform-aware paths: use `#[cfg(windows)]` / `#[cfg(unix)]` as needed
- Never mutate environment variables in tests; pass values explicitly

## Things to Always Check Before Proposing Changes

1. Is this already implemented in upstream?
2. Is there an open issue about this?
3. Would this conflict with an open PR?
4. Are there existing tests I should update/add to?
5. Does this need documentation updates?
