# TS-shaped Program adapter for emitter compatibility

The Rust port reaches emitter compatibility by exposing the napi `check()` output through a JavaScript-side adapter that mimics TS's `Program` / `Type` interfaces, rather than by porting the emitter framework to Rust.

The original plan listed emitters as a non-goal because their framework is deeply entangled with the JS ecosystem and user code. As the project completed M1–M7, the question of "what's the path to emitter compatibility?" became practical. Three options were considered: (A) leave emitters unsupported, (B) build a JS adapter that presents Rust output as TS-shaped Programs, or (C) port the emitter framework plus a target emitter to Rust. We chose (B) because it enables existing TS emitters with a bounded effort (~600–1,200 LOC of adapter), preserves the Rust port's "checked type graph" achievement as the output boundary, and avoids re-implementing the asset-emitter framework that itself depends on Node.js file APIs. The adapter strictly targets navigator-style emitters; full Program interface fidelity (mutable graphs, projections, library/source loading) is explicitly out of scope.

## Considered Options

- **(A) Leave emitters unsupported** — Rejected. Reasonable stopping point but leaves the Rust port without a clear path to "compile a real TypeSpec spec end-to-end." Doesn't add learning value.
- **(C) Port emitter framework + a real emitter** — Rejected. ~3,000 LOC for the framework plus ~5,000 LOC for an emitter like OpenAPI3, much of which is asset-emitter scaffolding bound to Node's `fs` APIs. Wrong tradeoff for a learning project — would be re-implementing JS infrastructure rather than learning new Rust/FFI patterns.

## Consequences

- The Rust port's "edge" remains the checked type graph. Emitters live in JS, consume Rust output through the adapter.
- Type identity (`===`) preservation in the adapter is load-bearing — emitters use `WeakMap<Type, ...>` extensively. A single `Map<TypeId, JsObject>` cache during materialization is the implementation lever.
- Decorator-derived state (M6) flows through the adapter via decorator helpers (`getDoc`, etc.) that read the M6 side table. Any new decorator added later needs a corresponding helper exposed in the adapter.
- Real-world emitters (JSON Schema, OpenAPI3) will likely surface gaps in the adapter API (mutable graphs, missing helpers); each gap becomes a follow-up scoped to the emitter being supported. The test emitter validates the navigator-style core; broader compatibility is incremental.
