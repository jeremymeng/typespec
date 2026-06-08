# Path A: replace only the type-checker; keep TS for CLI, host, libraries, and emitters

To make real TypeSpec emitters runnable end-to-end (`tsp compile main.tsp --emit @typespec/json-schema`), the Rust port replaces only the type-checking pipeline (scanner/parser/binder/checker). The TS `@typespec/compiler` package retains responsibility for the `tsp` CLI, `tspconfig.yaml` parsing, the file-system host, module resolution as a default implementation, library packages, decorator JS implementations, the `$onEmit` invocation contract, and the asset-emitter framework. Milestones M9–M13 implement this division.

The alternative (Path B — a parallel `tsp-rust` Rust binary that embeds a JS runtime to call `$onEmit`) was considered and rejected. Path A preserves all of the Node-based ecosystem (which is what real emitters use) and reuses existing TS code rather than reimplementing it. The Rust port's contribution is the type-checking algorithm, not the surrounding infrastructure.

This decision shapes M9–M13:

- M9 introduces **JS callbacks for I/O and module resolution**. Rust never calls `std::fs` for compiler inputs.
- M10 loads the stdlib **via JS host resolution** — the stdlib `.tsp` files come from `node_modules/@typespec/compiler/lib`, not embedded as Rust resources.
- M11 **registers existing JS decorator implementations** with M6's callback bridge. The Rust side validates declarations and target/arity; the JS side performs the actual decorator effects via the existing TS code.
- M12 makes the Program adapter satisfy what `@typespec/json-schema` calls — the adapter expands toward the TS `Program`/`Checker` surface but never claims full fidelity. Each emitter integration is incremental.
- M13 wires the Rust path into the existing `tsp compile` CLI as an opt-in flag (`--use-rust-checker` / `TYPESPEC_USE_RUST_CHECKER=1`). The default remains TS.

## Considered Options

- **Path B: parallel Rust `tsp-rust` binary** — Rejected. Requires embedding a JS runtime to call `$onEmit`. Either spawn Node as a subprocess (process boundary every emit), use `deno_core`/`rusty_v8` (massive dependency, sandbox concerns), or call into Node from a Rust-host napi consumer (rare pattern, fragile). All of these reimplement infrastructure that already works in TS.
- **Port the emitter framework to Rust** — Rejected. ~3,000 LOC for the framework plus thousands more per emitter. Reimplements JS infrastructure rather than learning new Rust patterns. Conflicts with the project's learning goal.

## Consequences

- The Rust port's "edge" is the checked type graph. Everything after the checker (file writing, library system, emitter invocation, CLI) stays JS.
- Performance gains apply to type checking only. Whether that's the bottleneck for any given spec is workload-dependent.
- Rust-checker mode is opt-in. The TS path remains the default and is what most users will run for the foreseeable future.
- Adding a new library (e.g., `@typespec/sse`) requires registering its decorators via the M11 registration helper. New emitters may surface adapter API gaps that are addressed incrementally per emitter.
- Failure modes for Rust-checker mode must include a clear path back to TS-checker (the user can drop the flag). Diagnostics from Rust-checker should clearly identify themselves as such.
