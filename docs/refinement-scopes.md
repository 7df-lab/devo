# Refinement scopes

`/refine` and `await refine.run()` consolidate the current session harness.
They do not automatically read the full conversation or make local notes global.
Automatic interval refinement remains local.

Use `/refine --global [instructions]` or
`await refine.run(instructions, global_=True)` to explicitly request global
refinement. The existing global editor retains its previous input and edit contract. A
separate create-only planning request sees a bounded, read-only snapshot of
current-session memories and global deduplication context. It may distill
reusable lessons into global memories; all its edits must cite a source. The
two proposals commit atomically, or neither is applied. It does not move or delete session entries.

The source snapshot is limited to 16 memories / 12,000 characters; each memory
must be at most 2,000 characters. Oversized entries, obvious secrets, and memories
marked with unverified trust are excluded rather than truncated into misleading
fragments. This is defense in depth, not comprehensive secret detection. Review
session memories before explicitly requesting cross-session reuse.

For a source-derived new memory, the planner supplies `after.source_id` and a
short `after.applicability`. The host accepts only memory Create edits for such
sources, preserves source project qualifiers, and records source identity and a
content fingerprint. This does not create executable skills or grant privileges.
A frozen source snapshot documents what was reviewed; later local changes are
not silently included in the same plan. Model judgment is not proof that a
lesson is generally true or improves performance.

Already promoted source content is not re-submitted. Host-derived IDs prevent
concurrent promotions of the same source from overwriting each other; a racing
commit fails the existing transactional drift check instead. Global updates,
deletes and unrelated entries retain the existing re-read-before-apply checks.
Refinement history and edits commit atomically, and `/refine --global rollback
<refinement-id>` can undo the global refinement if its post-image is unchanged.

Global state lives in `<DEVO_HOME>/harness/harness_state.json`. Subsequent turns
of persistent sessions using that same Devo home read it alongside their local
state. Local entries win on an ID collision. This is not cross-machine syncing.

## Reference design

Prime Agent (`PrimeIntellect-ai/prime-agent`, inspected commit
`d649b93cfe94d1bbc8c7517bd24899a89d998ebf`) uses explicit global scope, a
read-only planning context and a target-store baseline revalidated at apply.
Its global planner consumes trajectory as well as global state; it does not
itself copy the session harness into global state. Devo uses bounded session
memory evidence here instead of expanding access to the full conversation.
