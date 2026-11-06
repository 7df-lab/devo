# Bundled skills

`assets/bundled/` is the only authored bundled skill tree. Keep each skill's
instructions, Python sources, package metadata, and references together there.
The build script requires that tree and embeds it into `devo-skills`; it does
not fall back to sample assets or another crate's copy.

`SkillsManager` installs the embedded tree into
`<devo_home>/skills/.system/` before catalog discovery. Installed skill files
retain their source-relative paths. The kernel imports enabled packages from
the selected catalog entries, so release packaging must not copy additional
skill modules into `runtime/python-site/`.

The release Python site contains the kernel runtime and the third-party
dependencies pinned in `scripts/runtime/requirements.lock`. Embedding a skill
does not install its `pyproject.toml` dependencies. When adding a dependency to
a bundled skill, check the release requirements as well as the development
environment.

Regression checks:

```sh
cargo test -p devo-skills
bun test scripts/runtime
```

The installation test compares all canonical files against their embedded,
installed copies. Runtime packaging tests guard against the retired duplicate
trees and a second skill import path in `python-site`.
