# Spike: napi-rs Round-trip for Arena-Allocated AST

**Status**: Complete
**Date**: 2026-06-06
**Decision**: Hybrid — eagerly serialize a flat AST node table for full parse results, keep lazy `External<T>` handles for sparse/debug/checker-style queries.

## Setup

Built a disposable spike crate at `crates/_spike_napi_roundtrip` and JS-side spike tests at `packages/compiler-rust/spike`.

Reproduce:

```powershell
pnpm -C packages\compiler-rust build:spike
Set-Location packages\compiler-rust
node --expose-gc ..\..\node_modules\vitest\vitest.mjs run spike\roundtrip.bench.test.ts spike\lifetime.test.ts
```

The mini-AST uses `NodeId = u32`, a `MiniArena`, and five variants: `Module`, `ModelDecl`, `Property`, `TypeRef`, and `Identifier`. The benchmark shape is 834 models × 5 properties, producing 10,011 nodes.

## Q1: Serialization cost (10K nodes)

Measured on Windows with Node `--expose-gc`, 10 iterations, `performance.now()`, and `process.memoryUsage()` before/after build.

| Approach | Build (ms, median) | Build (ms, p95) | Traversal (ms, median) | Traversal (ms, p95) | Heap delta (MB, median) | External delta (MB, median) |
| -------- | -----------------: | --------------: | ---------------------: | ------------------: | ----------------------: | --------------------------: |
| Eager    |             30.589 |          51.516 |                  0.366 |               1.606 |                   1.677 |                           0 |
| Lazy     |              0.817 |           1.522 |                 32.045 |              41.099 |                 ~0.0004 |                           0 |

Ratios from the same run:

- Eager build/marshal was ~37.4× slower than lazy handle creation.
- Lazy full traversal was ~87.6× slower than eager JS traversal because it performs one napi call per node.
- Eager heap growth was ~1.7 MB. Lazy JS heap growth rounded to zero, but native memory is held by the external handle. The spike passed an external memory size hint, but `process.memoryUsage().external` did not visibly move at this size.

Interpretation: eager serialization is acceptable for a 10K-node full parse result when JS will inspect most nodes. Lazy handles are excellent when JS needs only a few nodes, but they are a poor fit for full-tree traversal.

## Q2: External<T> ergonomics

Actual code used:

```rust
#[napi(js_name = "parseMiniLazy")]
pub fn parse_mini_lazy(
    env: Env,
    model_count: Option<u32>,
    props_per_model: Option<u32>,
) -> Result<JsExternal> {
    let arena = build_tree(
        model_count.unwrap_or(DEFAULT_MODEL_COUNT),
        props_per_model.unwrap_or(DEFAULT_PROPS_PER_MODEL),
    )?;
    let size_hint = arena.approximate_external_size();
    env.create_external(arena, Some(size_hint))
}

#[napi(js_name = "getNode")]
pub fn get_node(handle: External<MiniArena>, id: u32) -> Result<NodeInfo> {
    node_info(handle.as_ref(), id)
}
```

Findings:

- `MiniArena` can be stored directly. No `Arc<MiniArena>` or `RefCell<MiniArena>` is needed for read-only lazy queries.
- Function parameters use `External<MiniArena>` directly; `handle.as_ref()` gives `&MiniArena`.
- napi-rs tags externals by Rust `TypeId`. Passing an `External<WrongExternal>` to `getNode` threw: `T on get_value_external is not the type of wrapped object`.
- Friction: in napi-rs 2.16, `Env::create_external` returns deprecated `JsExternal`; `External<T>` is the preferred bindgen type. The combination still works, but the API shape is mildly confusing.

## Q3: Lifetime semantics

Observed with `node --expose-gc`:

1. Creating a lazy arena and holding the JS handle kept the arena alive.
2. Holding the handle while doing 1,000,000 JS dummy allocations preserved the arena; `getNode(handle, root)` still returned `Module` and the Rust `Drop` counter stayed at 0.
3. Dropping the last JS handle reference, forcing GC, and checking a `WeakRef` produced `weak.deref() === undefined` and Rust drop count `1` in a direct probe. There is no usable “invalid handle” to pass back once JS has released it.
4. If GC does not collect immediately, the handle remains valid; this is normal V8 non-determinism.
5. Posting the external handle to a Node worker threw a structured-clone error (`could not be cloned`). The same handle cannot cross worker boundaries as-is.

napi-rs guarantee observed from source: external values are finalized via `raw_finalize::<T>`, and `External<T>` validates the stored Rust `TypeId` when converting from a napi value. Footgun: JS can keep numeric `NodeId`s after the handle is gone; APIs must require the arena handle for every lookup and never expose borrowed Rust references as JS-owned data.

## Q4: Ergonomic friction

- Boilerplate is noticeable even for five node variants. A real 50+ variant AST should use generated serializers or a compact node table format.
- Error messages for wrong external types were clear. Build errors around napi return types were less obvious, especially `JsExternal` vs `External<T>`.
- Read-only `External<T>` did not fight the borrow checker. The hard Rust problem was self-referential `bumpalo` data: storing `&str` allocated from a `Bump` owned by the same struct is awkward. The spike used a localized unsafe lifetime extension; production should prefer interned string IDs, source spans, or owned strings unless a safer arena pattern is introduced.
- Rich enum payloads round-trip cleanly as a flat `{ id, kind, children, payload }` node table. Deep recursive JS object graphs would be more expensive and harder to version.

## Decision and Rationale

**Choice**: Hybrid.

For M3 parser parity, return an eager, flat serialized AST node table by default. The comparison tests need to inspect the whole tree, and the benchmark shows full-tree lazy traversal spends far more time crossing the napi boundary than the eager path spends serializing.

Keep the lazy `External<T>` handle pattern available for sparse queries and later checker/decorator scenarios where JS may ask for a handful of nodes or types. Lazy handle creation is very cheap and keeps JS heap pressure low, but it should not be the primary full-AST traversal API.

The production M3 shape should avoid a deeply nested JS object graph. A flat node table with `NodeId` references preserves the arena model, keeps marshalling predictable, and matches the future checker's need to avoid exposing borrowed Rust references.

Revisit if parser ASTs become much larger than this spike or if JS consumers only need tiny slices of the AST. In that case, a lazy-first parser API plus batched `getNodes(ids)` could be worth another measurement.

## Impact on M3 plan

Updated the M3 section of `plan.md` to say the parser API should use a hybrid design: eager flat `SerializedAST` for `parse(source)` and optional lazy handles for sparse/debug access. No task dependency changes were needed.
