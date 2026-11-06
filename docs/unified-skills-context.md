# Unified Skill loading and host-owned harness state

## Ownership

- `crates/skills/assets/bundled` is the single authored source for bundled Skill instructions and Python code. The Rust build embeds this tree and the catalog installs it under the Devo home.
- Rust `SkillCatalog` owns discovery, canonical path identity, enablement, and selection. The Python bootstrap receives the catalog's enabled bundled package manifest. It does not scan `.system`, assume a fixed import list, fall back to development stubs, or resolve selected modules from ambient `PYTHONPATH`.
- Only bundled packages are automatically executed. Discovering user, workspace, or plugin instructions does not authorize automatic Python import. Their documented execution remains explicit.
- Rust owns persistent harness reads and mutations. Python carries requests and holds a cached view. Harness `skill` entries are reusable instructions/reference records, not executable catalog packages or capability grants.

## Turn boundary behavior

At a product turn boundary, refresh the available-Skill instructions and Python manifest from the same catalog. Sync enabled bindings in the existing kernel without clearing user variables or resetting package state. The prompt reports only successful bindings, so missing optional dependencies are not described as pre-imported modules.

When a parked cell owns the execution stream, selection refresh is deferred without waiting or interrupting it. The next turn omits pre-import claims until an idle-boundary refresh succeeds.

Disabling removes host-owned automatic bindings and module registrations at the next idle turn boundary. It does not delete shared installed assets, revoke existing Python references, cancel running work, or turn the trusted Python process into a sandbox. Re-enabling imports the selected package again. An explicit user replacement of an existing binding is preserved until that package identity changes.

The standalone CLI uses its effective workspace configuration. The discrete `skill` tool uses the same catalog lookup and ambiguous-name/disabled checks instead of recursively searching arbitrary workspace files.

## Harness API migration

```python
await rlm.harness.load()  # refresh before reading old or externally refined state
print(rlm.harness.overview())
entry = await rlm.harness.create_memory("Tests", "Run the focused test first.")
await rlm.harness.update_memory(entry.id, entry.title, "Run focused tests, then integration tests.")
await rlm.harness.delete_memory(entry.id)
```

`get`, `list`, and `overview` remain synchronous cached reads. Persistent create/update/upsert/delete/refinement calls are now awaitable. This is an explicit API migration: the supported host bridge is asynchronous, and maintaining synchronous writes would require a second transport or unsafe event-loop re-entry. There is no fallback to direct file writes. Use explicit in-memory state for temporary data; its mutations also use `await` for a consistent interface. Direct `save()` of a persistent snapshot and custom filesystem paths now raise migration errors.

The host resolves local/global destinations from session identity; Python cannot pass an arbitrary store path. Mutations pass host permission checks and are rejected in Plan mode. Ephemeral sessions have no persistent local store. Global writes require the explicit global scope and the normal host permission decision.

Host CRUD and `/refine` share a short read-modify-write transaction under a cross-process advisory sidecar lock. Refinement scheduling only queues work; storage initialization and failures occur during apply. Permission checks and model calls occur outside the lock. Unique temporary files provide atomic replacement and preserve existing permissions. Corrupt state fails without replacing the file. Digests enter the next model request at a turn boundary, not the already-running request.

## Compatibility and limits

- Existing `config.toml`, Skill paths, enable/disable overrides, and persisted harness JSON are retained. Old copied source trees are removed from the repository, not user-authored stores.
- The retained legacy TypeScript refinement implementation is outside the Native Rust/Python execution path and is not migrated here. Its prompt examples use the current Python API.
- Generic MCP and authored Linear/Notion transports are separate work. This change does not alter their result/authentication contracts.
- Optional bundled dependencies still require provisioning. Missing imports are recorded as `_devo_<module>_import_error` and omitted from the pre-imported prompt list. This does not silently install additional packages.
- Arbitrary Python still runs as the user. Host facade checks are policy boundaries for supported operations, not OS enforcement against arbitrary Python code.
- The catalog's existing shared on-disk installer is retained; version upgrades and external filesystem edits are not a new hot code-reload mechanism.
