---
artifact_id: L2-DES-APP-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-21
---

# L2-DES-APP-001 — Memory-Efficient Rust Data Models

## Purpose

Refine the lightweight-operation requirement into technical design principles for Rust data models that may otherwise retain avoidable memory during large sessions or large deserialization workloads.

## Background / Context

Rust struct composition stores fields inline. For large composite structs, `Option<LargeStruct>` does not necessarily make the parent object small when the option is `None`; the parent may still reserve enough inline space for the large struct. This differs from reference-oriented object models where a nullable child object is usually represented by one pointer in the parent.

The program may deserialize, retain, and traverse large collections of sparse objects such as session records, tool events, model metadata, workspace indexes, or cached external data. Sparse optional substructures can therefore create avoidable memory pressure even when most nested content is absent.

## Design Requirement

For large or frequently repeated Rust data structures, the program should avoid inline storage of sparse optional substructures when that storage creates meaningful memory overhead. When a nested structure is often absent or semantically empty, the design should consider `Option<Box<T>>` or another indirection strategy so the parent object stores only a pointer-sized optional value when no nested content exists.

## Design Principles

- Treat memory layout as part of data model design for large collections, long sessions, and deserialized external data.
- Prefer simple inline structs for small, dense, frequently accessed data where boxing would add unnecessary allocation or pointer chasing.
- Consider `Option<Box<T>>` for large nested structs that are often absent, all-default, or all-empty.
- When deserializing sparse nested structs, detect semantically empty values and avoid retaining boxed objects that carry no useful information.
- Implement emptiness checks as explicit domain behavior, such as an `is_empty` method, rather than relying on incidental serialization details.
- Keep serialization and deserialization behavior compatible with the public data contract when changing internal storage representation.
- Measure memory impact before and after optimization instead of relying only on `size_of::<T>()`.
- Treat extra CPU cost, heap fragmentation, and pointer-chasing overhead as tradeoffs that must be justified by memory savings.

## Applicability

This design applies when all of the following are true:

- A Rust data type is stored many times, retained across turns, or loaded from a large data source.
- The type contains nested structs that are often semantically empty.
- Inline representation creates measurable or reasonably expected memory pressure.
- Boxing or another indirection strategy does not make the user-visible workflow slower or less reliable overall.

This design does not require boxing every optional nested structure.

## Serde Guidance

When using Serde for sparse nested data, a custom deserializer may deserialize the nested value, check whether it is semantically empty, and store `None` instead of `Some(Box<T>)` when no useful data exists.

Example shape:

```rust
#[serde(default, deserialize_with = "deserialize_boxed_value")]
pub value: Option<Box<SparseValue>>;
```

The serializer should preserve the expected external representation and avoid exposing internal boxing decisions as a wire-format change unless an explicit data-contract change is approved.

## Measurement and Verification

Memory optimizations must be measurable when they are introduced for performance reasons.

- Use targeted benchmarks or profiling scenarios that represent realistic large sessions or large data loads.
- Prefer allocator-level or process-level memory measurement for composite object graphs, because `size_of::<T>()` does not include heap allocations reachable through pointers.
- Keep memory profiling optional so normal builds do not require profiling allocators or extra runtime overhead.
- Record the before-and-after memory impact and the CPU or latency tradeoff in the implementation or verification notes.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-005 | 1 | specs/L1/L1-REQ-APP-005-lightweight.md | Provides technical design guidance for avoiding unnecessary memory growth. |
| specified-by | L3-DES-ARCH-001 | 4 | specs/L3/L3-DES-ARCH-001-crate-architecture.md | L3 defines crate ownership and efficient data boundary placement. |

## References

- https://dystroy.org/blog/box-to-save-memory/#about-rust-structs-and-memory

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-21 | Assistant | Initial | Initial draft from approved L2 memory-optimization discussion. |
