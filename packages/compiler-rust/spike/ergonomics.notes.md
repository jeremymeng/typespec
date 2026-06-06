# napi-rs round-trip spike ergonomics notes

Throwaway notes from the M1.5 spike. The durable writeup is `docs/rust-port/spike-napi-roundtrip.md`.

## External<T> snippets used

```rust
#[napi(js_name = "parseMiniLazy")]
pub fn parse_mini_lazy(env: Env, model_count: Option<u32>, props_per_model: Option<u32>) -> Result<JsExternal> {
    let arena = build_tree(model_count.unwrap_or(500), props_per_model.unwrap_or(5))?;
    env.create_external(arena, Some(arena.approximate_external_size()))
}

#[napi(js_name = "getNode")]
pub fn get_node(handle: External<MiniArena>, id: u32) -> Result<NodeInfo> {
    node_info(handle.as_ref(), id)
}
```

## Observations

- `MiniArena` can be stored directly in `External<T>` for read-only access; no `Arc` or `RefCell` was required.
- napi-rs tags externals with Rust `TypeId`; passing a different `External<T>` to `getNode` throws instead of silently miscasting.
- The self-referential `bumpalo` shape is awkward if nodes store `&str` allocated from a `Bump` owned by the same arena. The spike uses a localized unsafe lifetime extension; production should prefer offsets, interned IDs, or owned strings unless the arena owns nodes through a safer pattern.
- Returning many rich enum variants as JS objects is boilerplate-heavy. A generated serializer or a compact node table would be preferable for a 50+ variant AST.
