# Install a Skill in Devo

## Choose a discovery root

| Scope | Default destination |
| --- | --- |
| Current workspace | `<workspace>/.devo/skills/<name>/` |
| User | `<DEVO_HOME>/skills/<name>/` |

Use the effective `skills.workspace_roots` or `skills.user_roots` when customized. Relative workspace roots resolve under `<workspace>/.devo/` and relative user roots under `<DEVO_HOME>/`. If the repository already shares skills in `.agents/skills/`, follow its convention. Devo searches that directory from the session cwd up to the project root. `<DEVO_HOME>/skills/.system/` is reserved for Devo's bundled assets and may be replaced on startup.

## Prepare and place the package

1. For an existing package, use the exact local folder or fetch the requested repository revision into a staging directory outside the discovery roots. Locate the specific folder with `SKILL.md`. If there are multiple skills and the request identifies only one, install only the matching folder. Keep untrusted archives and links from writing outside the staging directory.
2. Confirm the UTF-8 entrypoint, frontmatter name, description, and relative resources. Read executable content and setup instructions before running anything. If an external skill has Python code, use its documented explicit command and environment. Only bundled system skills get automatic Python kernel bindings.
3. Copy the whole selected skill directory, including referenced files and any `agents/openai.yaml`. Preserve the directory layout and script executability where applicable. If you authored the package at its final destination, use it in place. Keep the existing copy intact on a conflict unless the user asked to update it.

Two skills with the same name at different paths can coexist in the catalog and make name-only invocation ambiguous. Check all visible entries for the name. Use an unambiguous name for new work when the name is flexible, or use the exact path in a picker or a path-based selector. Preserve any existing disabled rule unless enabling the skill was requested.

## Verify discovery and use

Check the expected `name`, `path`, and `enabled` state in the target workspace's skill catalog. In the TUI, use the skill picker. A Native client can refresh the catalog with `skill/list` using the target session's `cwd` and `forceReload: true`. If the running backend still shows the previous catalog after a file edit, reload the catalog before verifying the skill. For an existing disabled skill, the client can use `skill/set_enabled` with `cwd`, `path`, and `enabled` when enablement was requested.

Where available, load the new instructions with Devo's skill tool or use the skill picker. In a fresh view of the catalog, `$name` invokes a skill by name when the name is unambiguous. Test scripts or a representative workflow separately from discovery when safe. If you only verified the files on disk, report the runtime check as pending.
