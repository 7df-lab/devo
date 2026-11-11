# Native history paging cache

`session/items/list` and `session/turns/list` load canonical rollout history
through the same server cache. A stable journal up to 16 MiB can occupy the whole
existing cache budget. Previously a separate 4 MiB entry limit caused every page
of a 4–16 MiB journal to reparse the entire file.

The cache retains at most four logical-session entries, with a total charge of
16 MiB measured in source journal bytes. Entries are evicted least-recently-used
before admitting a new one. These are retention/accounting limits, **not a
strict heap or RSS ceiling**: parsed representations and active readers holding
an `Arc` can outlive eviction. Concurrent cold reads of different sessions also
have their own transient parsing allocations. This change does not increase the
existing total byte budget or entry count.

The key includes the logical session, journal path, file length, and modification
time. Readers check the file stamp before reusing an entry and again before
publishing a newly parsed projection. Same-session concurrent cold readers share
a load gate and reuse the completed cached result. Failed parses are not cached.
Append/rollback writes invalidate through the existing file-stamp mechanism.
This is not a version-pinned multi-page snapshot: concurrent journal changes can
still cause a new projection between pages, as before.

Journals larger than 16 MiB remain uncached. Page projection/cloning, full-history
fetching, UI rendering, and same-size rewrites with an indistinguishable file
stamp are outside this narrow change.

## Verification

Run the focused tests and optional parser-work benchmark:

```sh
cargo test -p devo-server --lib history_cache
cargo test -p devo-server --lib benchmark_large_history_page_loads -- --ignored --nocapture
```

The benchmark generates 6,000 canonical assistant items with 1 KiB text each.
It compares 30 complete uncached reads (the repeated parse work incurred above
the former cap) with 30 cache loads. It is not an end-to-end RPC or TUI benchmark,
and does not assert a wall-clock threshold in CI. Regression tests separately
check projection equality, single-parse concurrent reuse, append invalidation,
uncached oversized journals, byte-budget eviction, and the entry-count limit.
