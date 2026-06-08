# Plan: Port TypeSpec Compiler to Rust (Phase 1+2)

## Goal

Port the TypeSpec compiler's scanner, parser, binder, and checker from TypeScript to Rust as a learning project. The Rust implementation lives side-by-side with the existing TS compiler in the monorepo, connected via napi-rs bindings, validated by comparison tests and dual-runner tests.

> **Note on plan location**: This plan is committed to the repo at `docs/rust-port/plan.md` so it can be referenced by handoff agents (cloud, other contributors, future-you). The ADR for arena allocation lives at `docs/adr/0001-arena-based-type-graph.md`.

## Resolved Decisions

| Decision                 | Choice                                                                                                                                                     | Alternatives Rejected                                 |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| Primary goal             | Learning Rust                                                                                                                                              | Production migration, performance                     |
| Location                 | In monorepo under `crates/` + `packages/compiler-rust`                                                                                                     | Standalone repo                                       |
| Scope                    | Scanner + Parser + Binder + Checker                                                                                                                        | Scanner/Parser only; full compiler including emitters |
| AST/type representation  | Arena allocator with index-based references (`bumpalo`)                                                                                                    | `Rc<RefCell<T>>`, raw pointers                        |
| napi-rs API surface      | Fine-grained per-phase (`scan`, `parse`, `check`)                                                                                                          | Coarse `compile()` only                               |
| Decorator handling       | JS callbacks from Rust via napi-rs                                                                                                                         | Skip decorators; reimplement in Rust                  |
| Test strategy            | Comparison tests + dual-runner where tests aren't TS-specific                                                                                              | Rewrite tests in Rust; comparison only                |
| Stdlib/intrinsics        | Hard-code intrinsics in Rust, parse stdlib `.tsp` files                                                                                                    | Load everything from JS                               |
| Scanner approach         | Hand-written (matching TS design)                                                                                                                          | Use `logos` crate                                     |
| `SourceFile` abstraction | Replicate in Rust — scanner owns line/character mapping                                                                                                    | Pass raw strings, reconstruct positions on JS side    |
| napi-rs threading        | Synchronous on main JS thread — `check()` is invoked from JS and stays there; decorator callbacks are direct `JsFunction` calls (not `ThreadsafeFunction`) | Async checker with `ThreadsafeFunction`               |
| CI / prebuilt binaries   | Use `@napi-rs/cli`'s standard cross-platform build matrix; publish prebuilds per platform                                                                  | Build from source on install; single-platform support |
| Key crates               | `napi`, `napi-derive`, `napi-build`, `bumpalo`, `string-interner`                                                                                          | `typed-arena`, `logos`, `miette`                      |

## Cross-cutting Design Constraints

These decisions affect multiple milestones — call them out so agents working on individual milestones don't accidentally violate them.

### `SourceFile` abstraction

The Rust scanner takes ownership of source text and computes line/character positions internally:

```rust
pub struct SourceFile {
    text: String,
    line_starts: Vec<u32>,  // Computed once on construction
}

impl SourceFile {
    pub fn new(text: String) -> Self { /* compute line_starts */ }
    pub fn line_and_character(&self, pos: u32) -> (u32, u32) { /* binary search */ }
}
```

The napi binding wraps this and exposes line/character to JS. **Do not** pass raw strings to JS and recompute positions there — the scanner needs efficient access during error reporting.

### Threading model for napi-rs callbacks

The Rust checker runs synchronously on the main JS thread. JS decorator callbacks are invoked via `napi::JsFunction::call()` (not `ThreadsafeFunction`). Rationale:

- The TS checker is synchronous — we mirror that
- napi calls from JS into Rust happen on the main JS thread by default
- A synchronous `JsFunction::call()` works in this context and avoids the complexity of async/threadsafe callbacks
- **Constraint**: This means the checker cannot do parallel work that needs to call JS. If we ever want parallelism (e.g., parallel type checking of independent files), this design must be revisited.

### Position representation

Use `u32` for source positions and node IDs throughout. TypeSpec source files are well under 4GB; `u32` is half the memory of `usize` on 64-bit platforms and matches what `rustc` uses.

### Error handling strategy

- **Parser/scanner errors**: Recoverable — produce a `Diagnostic` and continue. Match TS error recovery behavior so diagnostic output is identical.
- **Checker errors**: Produce diagnostics, don't panic. Use `Result<T, Diagnostic>` for fallible operations.
- **Internal invariant violations**: Use `panic!` with a clear message — these are compiler bugs that should be loud.
- Use the `anyhow` crate sparingly (only for napi boundary error reporting); prefer typed errors internally.

## Repository Layout

```
typespec/
├── crates/                              ← NEW: Cargo workspace
│   ├── Cargo.toml                       ← Workspace root
│   ├── rust-toolchain.toml              ← Pinned Rust version
│   ├── typespec-source/                 ← Shared: SourceFile, positions, diagnostics
│   │   └── src/lib.rs
│   ├── typespec-scanner/                ← Scanner crate (1,560 LOC TS equivalent)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── token.rs                 ← Token enum + keyword mapping
│   │       └── charcode.rs              ← Character classification
│   ├── typespec-parser/                 ← Parser crate (3,085 LOC TS equivalent)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ast.rs                   ← AST node types (arena-allocated)
│   │       └── syntax_kind.rs           ← SyntaxKind enum
│   ├── typespec-checker/                ← Checker crate (binder + checker + type-relations)
│   │   ├── Cargo.toml                     (~11K LOC TS equivalent)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── binder.rs                ← Name binding (649 LOC equiv)
│   │       ├── name_resolver.rs         ← Name resolution (1,226 LOC equiv)
│   │       ├── checker.rs               ← Type checker (8,062 LOC equiv)
│   │       ├── type_relation.rs         ← Type relation checking (1,122 LOC equiv)
│   │       ├── types.rs                 ← Type definitions (arena-allocated)
│   │       └── intrinsics.rs            ← Built-in intrinsic types
│   └── typespec-napi/                   ← napi-rs bindings
│       ├── Cargo.toml
│       ├── build.rs
│       └── src/
│           ├── lib.rs
│           ├── scanner.rs               ← scan() → token stream
│           ├── parser.rs                ← parse() → serialized AST
│           └── checker.rs               ← check() → diagnostics + types
├── packages/
│   ├── compiler/                        ← Existing TS compiler (unchanged)
│   └── compiler-rust/                   ← NEW: npm package wrapping napi bindings
│       ├── package.json
│       ├── index.js                     ← Re-exports napi build artifacts
│       ├── scripts/
│       │   └── build.js                 ← Triggers `napi build`
│       └── test/
│           ├── comparison/              ← Output comparison tests (Rust vs TS)
│           └── dual-runner/             ← Dual-runner tests (shared test logic)
├── .github/workflows/
│   └── rust-port.yml                    ← NEW: Rust build + prebuild matrix
└── docs/
    ├── rust-port/
    │   └── plan.md                      ← This file
    └── adr/
        └── 0001-arena-based-type-graph.md
```

## CI and Build Infrastructure

### Local development loop

- `cargo build` in `crates/` builds the Rust code only (fast)
- `cd packages/compiler-rust && pnpm build` invokes `napi build` to produce the `.node` native module
- `pnpm test` in `packages/compiler-rust` runs comparison and dual-runner tests
- `cargo test` in `crates/` runs pure Rust unit tests (fast inner loop — use this during scanner/parser development)
- **Iteration tip**: During Rust-only development, prefer `cargo test` over rebuilding the napi module. The full napi rebuild adds ~30-60s per change.

### CI

- A new GitHub Actions workflow `rust-port.yml` runs:
  - `cargo fmt --check`, `cargo clippy`, `cargo test` on Ubuntu only (fast feedback)
  - `napi build` + comparison tests on Ubuntu, macOS, Windows (full matrix)
- The existing TS CI is unaffected — the Rust workflow is independent
- Prebuilt `.node` binaries are published per-platform via `@napi-rs/cli`'s standard mechanism (`napi prepublish`). For a learning project this is overkill initially — start with build-from-source on install, add prebuilds when M2 ships.

### Toolchain pinning

`crates/rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.75.0"  # or current stable at start
components = ["rustfmt", "clippy"]
```

## Kill Criteria

Since this is a learning project, define explicit decision points. At the end of each milestone, ask yourself:

1. **Did I learn something valuable in this milestone?** If no for 2 milestones in a row, stop.
2. **Does the next milestone still sound fun and educational?** If no, stop or skip ahead.
3. **Is the napi-rs ergonomics still tolerable?** If you find yourself hating the bindings layer, consider abandoning the SxS approach and continuing as a pure Rust learning exercise.

Suggested natural stopping points:

- **After M2 (Scanner)** — you've learned Rust basics, napi-rs basics, ownership for simple cases. Honorable place to stop.
- **After M3 (Parser)** — you've tackled arena allocation, recursive descent in Rust, complex AST design. Major learning milestone.
- **After M5d (Type relations)** — you've grappled with cyclic graphs and complex type theory in Rust. Most of the learning value is captured.
- **After M6 (Decorator callbacks)** — full FFI mastery achieved.

**Explicit non-goals** (do not pursue, even if tempting):

- Production performance benchmarks beating the TS compiler
- WASM compilation target
- Port of the formatter, linter, emitter framework, language server, or CLI
- Replacing the TS compiler in any user-visible way

## Milestones

Each milestone is independently deliverable and testable. Agents can work on them sequentially.

---

### M1 — Project Scaffolding

**Goal**: Set up Cargo workspace, napi-rs build pipeline, and the `compiler-rust` npm package with a trivial "hello" binding that proves the JS↔Rust bridge works.

**Deliverables**:

- `crates/Cargo.toml` (workspace definition)
- `crates/rust-toolchain.toml` (pinned Rust version)
- `crates/typespec-napi/` with a trivial `#[napi] fn hello() -> String`
- `packages/compiler-rust/package.json` with build script calling `napi build`
- A single vitest test in `packages/compiler-rust/test/` that calls `hello()` and asserts the return value
- `.gitignore` entries for Rust build artifacts (`target/`, `*.node`)
- `.github/workflows/rust-port.yml` with `cargo fmt --check`, `cargo clippy`, `cargo test`, and `napi build` on Ubuntu

**Acceptance criteria**:

- `cargo build` succeeds in `crates/`
- `pnpm build` in `packages/compiler-rust` produces a `.node` native module
- `pnpm test` in `packages/compiler-rust` passes the hello-world test
- CI workflow runs green on a sample PR

**Estimated complexity**: Small (setup/config only)

---

### M1.5 — napi-rs Round-trip Spike

**Goal**: Before committing to the per-phase API design, validate that napi-rs can efficiently move arena-allocated AST-like structures across the JS boundary. This is a 1-day spike, not a production milestone.

**What to build**:

- A toy "mini-AST" in Rust: ~5 node types, arena-allocated with `bumpalo`, with parent/child references via `NodeId`
- A napi binding that exposes:
  - `parse_mini(source: String) → MiniAst` — returns the whole tree to JS
  - `get_node(id: u32) → NodeInfo` — lazy accessor for individual nodes
- Two test approaches in JS:
  - **Eager**: receive the whole tree as a JS object graph, traverse it
  - **Lazy**: receive just a root handle, call back into Rust for each child

**Questions this spike must answer**:

1. How expensive is serializing a 10K-node arena to JS objects? (Affects M3 API design)
2. Does the lazy handle pattern work cleanly with napi-rs `External<T>`?
3. How does arena lifetime interact with JS object lifetime — can we free the arena while JS still holds references?
4. Are there ergonomic issues with passing arena-allocated structs across the FFI boundary?

**Output**:

- A short writeup in `docs/rust-port/spike-napi-roundtrip.md` summarizing findings
- Decision: eager serialization, lazy handles, or hybrid? Documented in plan if it changes M3.
- If the spike reveals a blocker (e.g., serialization is unacceptably slow), revisit the per-phase API decision before M3.

**Estimated complexity**: Small (1 day, throwaway code)

---

### M2 — Scanner Parity

**Goal**: Port the TypeSpec scanner from TS to Rust. Expose `scan(source: string) → TokenInfo[]` via napi-rs. Validate against existing scanner tests.

**Key source files to port from**:

- `packages/compiler/src/core/scanner.ts` (1,560 lines)
- `packages/compiler/src/core/charcode.ts` (character classification)

**Deliverables**:

- `crates/typespec-source/src/lib.rs` — `SourceFile`, `Position`, `Diagnostic` (shared by all crates)
- `crates/typespec-scanner/src/lib.rs` — `Scanner` struct with `scan()` method
- `crates/typespec-scanner/src/token.rs` — `Token` enum matching all tokens in TS `Token` enum
- `crates/typespec-scanner/src/charcode.rs` — character classification functions
- `crates/typespec-napi/src/scanner.rs` — napi binding: `scan(source: String) → Vec<TokenInfo>`
- Comparison tests in `packages/compiler-rust/test/comparison/scanner.comparison.test.ts`
- Pure Rust unit tests in `crates/typespec-scanner/src/`

**Implementation guidance**:

- The TS scanner is a stateful, single-pass scanner. Mirror this design in Rust.
- `Token` is a numeric enum in TS — use a Rust `#[repr(u32)]` enum.
- The scanner handles string templates, multi-line strings, numeric literals, doc comments. Port all of these.
- Use `string-interner` crate for identifier interning.
- The TS `createScanner` returns an object with methods like `.scan()`, `.getTokenText()`, `.getTokenValue()`. In Rust, this becomes a `Scanner` struct with methods.

**Test strategy**:

- Create comparison tests that call both `createScanner` (TS) and `scan` (Rust) with the same inputs
- Compare: token types, token text, token positions (line/character)
- Use test inputs from `packages/compiler/test/scanner.test.ts` (468 lines of test data)
- Also generate fuzz inputs: random TypeSpec snippets, edge cases (empty strings, unicode, unterminated strings)

**Acceptance criteria**:

- Rust scanner produces identical token streams (type + text + position) for all inputs in scanner.test.ts
- Rust scanner handles all edge cases: string templates, multi-line strings, doc comments, numeric literals, unicode identifiers
- Diagnostic messages for malformed inputs match TS scanner diagnostics

**Estimated complexity**: Medium (1,560 LOC port + napi bindings + tests)

---

### M2.5 — Upgrade @napi-rs/cli to v3 (with ESM output)

**Goal**: Upgrade `@napi-rs/cli` from v2.x to v3.x and switch the generated bindings from CommonJS to ESM. Align with the rest of the TypeSpec monorepo, which is all ESM. Do this before M3 so the parser's larger surface area is built on the modern toolchain.

**Background / spike findings** (validated against current code on 2026-06-06):

- `--esm` flag exists in `@napi-rs/cli` v3.x only (not v2.x)
- Requires the Rust `napi` and `napi-derive` crates to also be on v3 — v2 Rust crates produced an empty `.d.ts` and no JS binding with the v3 CLI
- `napi-build` stays on `2.x` — **there is no v3 release**; `napi-build 2.3.2` is the latest and is intentionally written to work with both v2 and v3 CLIs by emitting both legacy and new `cargo:rerun-if-env-changed` env-var names. It's a small orthogonal crate (one `pub fn setup()` that emits platform-specific linker directives) whose API hasn't needed a major bump.
- v3 CLI replaces v2's verbose per-platform `switch` loader with a single-file ESM binding (`import { createRequire } from 'node:module'` + `export { ... }`); the native `.node` is still loaded via `createRequire` (Node platform limitation, not napi-rs)
- v3 CLI also generates a proper `.d.ts` with full type info (struct/enum/function signatures) — v2 in our setup produced 0 bytes
- v3 deprecates `napi.name` → `napi.binaryName` and `napi.triples` → `napi.targets` in `package.json`
- v3 CLI args differ from v2: `--cargo-cwd` → `--manifest-path`; output defaults to the **crate folder** unless `--output-dir` is specified

**Deliverables**:

- `packages/compiler-rust/package.json`:
  - `devDependencies["@napi-rs/cli"]`: `"^3"`
  - `"type": "module"`
  - `"main": "index.js"` (v3 emits ESM into `index.js` when `type: module`)
  - `"files"`: include `index.js`, `index.d.ts`, `*.node`
  - `napi.binaryName` instead of `napi.name`; `napi.targets` instead of `napi.triples`
  - `scripts.build`: includes `--esm --manifest-path ../../crates/typespec-napi/Cargo.toml --output-dir .`
  - Apply equivalent treatment to `scripts.build:debug`
  - `scripts.build:spike`: bring along — either also upgrade the spike crate to v3 (preferred for consistency) or leave it pinned to v2 with explanatory comment (spike is throwaway)
- `crates/typespec-napi/Cargo.toml`:
  - `napi`: `"3"` (with `default-features = false` and existing feature set re-validated against v3)
  - `napi-derive`: `"3"`
  - `napi-build` stays on `"2"` — no v3 published; current `2.3.2` is bi-CLI compatible
- `crates/_spike_napi_roundtrip/Cargo.toml`: same treatment, OR a comment noting it's frozen on v2 for historical reference
- Any source-code changes required by napi v3 API differences (most v2 macros are compatible; spot-check `External<T>`, `JsObject`, error types, the `Env::create_external` signature)
- `packages/compiler-rust/test/`: update imports to ESM-style if needed (vitest handles both; verify no breakage)
- `.github/workflows/rust-port.yml`: re-validate the build step works with new CLI args
- `pnpm-lock.yaml`: regenerated via `pnpm install`

**Acceptance criteria**:

- `pnpm exec napi --help` resolves a v3 binary (`node -e "console.log(require('@napi-rs/cli/package.json').version)"` reports `3.x.y`)
- `pnpm -C packages/compiler-rust build` succeeds with **zero deprecation warnings** (`napi.binaryName` / `napi.targets` correctly migrated)
- Generated `index.js` contains `import` / `export` statements, not `require` / `module.exports`
- Generated `index.d.ts` is non-empty and includes all exported functions/types
- `pnpm -C packages/compiler-rust test` passes — all 51 scanner comparison tests still green
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all green
- `.github/workflows/rust-port.yml` runs green on a sample PR (or locally simulated)

**Out of scope**:

- Functional changes to scanner, source, or napi bindings — this is purely a toolchain upgrade
- WASM target enablement (separate decision)

**Estimated complexity**: Small (toolchain upgrade with API-compatibility spot-checks)

---

### M3 — Parser Parity

**Goal**: Port the TypeSpec parser from TS to Rust. Expose `parse(source: string) → AST` via napi-rs. Define the arena-based AST.

**Updated by M1.5 spike**: Use a hybrid napi-rs API. `parse(source)` should eagerly return a flat `SerializedAST` node table with `NodeId` references for full-tree parser comparison tests. Keep lazy `External<AstArena>` handles as an optional sparse/debug access pattern, not the default full-tree traversal API.

**Key source files to port from**:

- `packages/compiler/src/core/parser.ts` (3,085 lines)
- `packages/compiler/src/core/types.ts` (AST node type definitions)

**Deliverables**:

- `crates/typespec-parser/src/ast.rs` — Arena-allocated AST node types
- `crates/typespec-parser/src/syntax_kind.rs` — `SyntaxKind` enum matching TS
- `crates/typespec-parser/src/lib.rs` — `Parser` struct, `parse()` function
- `crates/typespec-napi/src/parser.rs` — napi binding: `parse(source: String) → SerializedAST` (eager flat node table); optional lazy handle helpers for sparse/debug queries
- Comparison tests + dual-runner tests

**Implementation guidance**:

- The TS parser is a recursive-descent parser. Mirror this design in Rust.
- AST nodes reference each other. Use `NodeId` (index into arena) instead of direct references.
- The TS `SyntaxKind` enum has ~130 variants — port them all.
- Parser error recovery: the TS parser recovers from errors by inserting missing tokens. Replicate this behavior so diagnostic output matches.
- Key AST node types to port: `TypeSpecScriptNode`, `ModelStatementNode`, `EnumStatementNode`, `UnionStatementNode`, `OperationStatementNode`, `InterfaceStatementNode`, `ScalarStatementNode`, `AliasStatementNode`, `NamespaceStatementNode`, and all expression nodes.
- The API shape follows M1.5's hybrid decision: eager flat node table for full parse results; lazy handles only for sparse/debug queries.

**Arena design**:

```rust
use bumpalo::Bump;

pub struct AstArena {
    bump: Bump,
    nodes: Vec<Node>,  // All nodes stored here
}

pub type NodeId = u32;  // Newtype-wrap in practice: pub struct NodeId(u32);

pub struct Node {
    pub kind: SyntaxKind,
    pub pos: u32,
    pub end: u32,
    pub data: NodeData,  // Enum of node-specific data
}
```

**Test strategy**:

- Comparison tests: parse same source with both compilers, compare AST structure (node kinds, positions, child relationships)
- Dual-runner: adapt tests from `parser.test.ts` that only check structure (not TS object identity)
- Snapshot tests: serialize Rust AST to JSON and snapshot it

**Acceptance criteria**:

- Rust parser produces structurally identical ASTs for all inputs in parser.test.ts
- Error recovery produces matching diagnostics
- All SyntaxKind variants are represented

**Estimated complexity**: Large (3,085 LOC port + AST design + napi serialization + tests)

---

### M4 — Binder + Name Resolution

**Goal**: Port the binder and name resolver. These create the symbol table that maps names to declarations.

**Key source files to port from**:

- `packages/compiler/src/core/binder.ts` (649 lines)
- `packages/compiler/src/core/name-resolver.ts` (1,226 lines)

**Deliverables**:

- `crates/typespec-checker/src/binder.rs` — Symbol creation, scope building
- `crates/typespec-checker/src/name_resolver.rs` — Name resolution across scopes
- `crates/typespec-checker/src/types.rs` — Symbol and Scope types (arena-allocated)
- napi bindings for name resolution queries (resolve a name → declaration location)

**Implementation guidance**:

- The binder walks the AST and creates `Symbol` objects for each declaration
- Symbols are organized into `Scope` trees (namespaces, models, interfaces create scopes)
- Name resolution handles: qualified names (`A.B.C`), using statements, augment decorators
- Use the same arena pattern: `SymbolId` and `ScopeId` as indices (newtype-wrapped)

**Test strategy**:

- Create tests that resolve names in TypeSpec sources and compare results with TS compiler
- Test: simple names, qualified names, imports, using statements, shadowing, duplicate detection

**Acceptance criteria**:

- Names resolve to the same declarations as the TS compiler
- Duplicate name diagnostics match
- Scope hierarchy matches TS for all test inputs

**Estimated complexity**: Medium (1,875 LOC port + symbol table design)

---

### M5 — Structural Type Checker (Subdivided)

The checker is too large to deliver as a single milestone. It's subdivided into M5a–M5e, each ~1–2K LOC of TS source equivalent. **Each sub-milestone is a separate handoff** and produces working, tested code that builds on the previous.

**Shared across M5a–M5e**:

- `crates/typespec-checker/src/checker.rs` — grows incrementally
- `crates/typespec-checker/src/intrinsics.rs` — built-in types (string, int32, etc.) — populated in M5a
- `crates/typespec-checker/src/types.rs` — `Type` enum, type arena — grows incrementally
- napi binding `check(source: String) → { diagnostics, types }` — grows in capability

**Constraint for all of M5**: No decorator evaluation. Decorators are recognized and validated structurally (correct arity, valid target) but not executed. Decorator execution is M6.

#### M5a — Models, Scalars, and Intrinsics

- Create `Type` enum with `Scalar`, `Model`, `ModelProperty` variants
- Populate intrinsics arena: `string`, `int8`–`int64`, `uint8`–`uint64`, `float32`, `float64`, `boolean`, `bytes`, `void`, `never`, `unknown`, `null`
- Walk AST and create model + scalar types from declarations
- Handle model property types (including circular references — this is where arena indices shine)
- **Test**: `model.test.ts`, `scalar.test.ts` (subset that doesn't use decorators)
- **Source**: ~1,500 LOC of checker.ts

#### M5b — Enums, Unions, Aliases

- Add `Enum`, `EnumMember`, `Union`, `UnionVariant`, `Alias` to `Type` enum
- Type creation for these declarations
- Handle alias expansion (alias may reference any type)
- **Test**: `enum.test.ts`, `union.test.ts`, `alias.test.ts`
- **Source**: ~1,500 LOC

#### M5c — Operations, Interfaces, Namespaces

- Add `Operation`, `Interface`, `Namespace` to `Type` enum
- Handle operation parameter types (which are anonymous models)
- Handle interface operation membership
- Namespace nesting and merging
- **Test**: `operations.test.ts`, `interface.test.ts`, `namespaces.test.ts`, `global-namespace.test.ts`
- **Source**: ~1,500 LOC

#### M5d — Type Relations and Assignability

- Port `type-relation-checker.ts` (1,122 LOC)
- Implement: subtype check, assignability check, identity check
- Handles intersections of intrinsic ranges, numeric literal-to-scalar narrowing
- **Test**: `relation.test.ts`, `intersections.test.ts`, `effective-type.test.ts`
- **Source**: ~1,500 LOC

#### M5e — Templates and Template Instantiation

- Template parameters, constraints, defaults
- Template instantiation with memoization (instantiating `Foo<string>` twice returns the same type)
- Template arg validation
- **Test**: `templates.test.ts`, `unused-template-parameter.test.ts`, `references.test.ts`
- **Source**: ~2,000 LOC — this is the trickiest part of the checker

**Acceptance criteria (cumulative through M5)**:

- All non-decorator checker tests produce identical diagnostics
- Type relation checks match TS for all test cases
- Templates instantiate correctly with structural identity preserved

**Estimated complexity**: Very Large overall, but each sub-milestone is Medium

---

### M6 — Decorator Callbacks via napi-rs

**Goal**: Wire up the napi-rs callback mechanism so the Rust checker can call JS decorator implementations during type checking.

**Deliverables**:

- Callback infrastructure in `crates/typespec-napi/src/checker.rs`
- JS-side decorator registry that Rust can call into
- Integration with the checker: when encountering `@decorator`, call the JS implementation
- Tests that verify built-in decorators (`@doc`, `@route`, `@visibility`) work through the bridge

**Implementation guidance**:

- napi-rs supports `JsFunction` for synchronous JS callbacks on the main thread (the threading model decision in Cross-cutting Constraints)
- The Rust checker needs a `DecoratorCallback` trait or function pointer that calls into JS
- The JS side registers decorator implementations via something like `registerDecorator(name, fn)`; Rust looks them up by name through a JS-side registry held as a `JsObject`
- Arguments need marshalling: Rust types → JS objects → Rust effects
- This is the most napi-rs-intensive milestone — expect to learn a lot about the FFI boundary

**Key challenge**: The TS checker calls `marshalTypeForJs` to convert internal types to JS-friendly objects. You'll need an equivalent in Rust that creates napi-rs `JsObject` values from your arena-allocated types. The lazy-handle pattern from the M1.5 spike may be relevant here.

**Test strategy**:

- Test specific decorators: `@doc("hello")` should attach documentation
- Compare decorator effects between TS and Rust compilers
- Test error cases: invalid decorator arguments, missing decorators

**Acceptance criteria**:

- Built-in decorators execute correctly through the Rust→JS→Rust bridge
- Decorator validation errors match TS output
- No memory safety issues with the callback mechanism

**Estimated complexity**: Large (napi-rs callback design + marshalling + integration)

---

### M7 — Replace f64 with arbitrary-precision Numeric for literal values

**Status when added**: M5b chose `f64` for `NumericLiteralType.value`, `EnumMemberValue::Numeric`, and intrinsic `NumericRange` bounds as a pragmatic simplification. This leaves latent precision bugs the current comparison suite does not surface (int64 above 2⁵³, uint64 boundaries, decimal128, exact decimal equality like `0.1 + 0.2`). M7 reverses that decision and aligns with TS's arbitrary-precision `Numeric` representation.

**Goal**: Introduce a `Numeric` value type that mirrors TypeSpec's `Numeric` (BigInt mantissa + decimal exponent + sign + decimal-digit count). Replace every `f64` use for literal values, enum-member values, and numeric range bounds. Marshal across the napi-rs boundary using napi v3's native `BigInt` support.

**Reference**: see [ADR-0002](../adr/0002-arbitrary-precision-numeric.md).

**Key source files to mirror from**:

- `packages/compiler/src/core/numeric.ts` — the `Numeric` class with `InternalData { n: bigint; e: number; d: number; s: 1 | -1 }` and parser
- `packages/compiler/src/core/numeric-ranges.ts` — intrinsic ranges (already ported to Rust in M5d, but with `f64` bounds)

**Deliverables**:

- New `crates/typespec-source/src/numeric.rs` (or in `typespec-checker` if you prefer; `typespec-source` is recommended since literals are foundational):
  - `pub struct Numeric { n: BigInt, e: i32, d: u32, s: i8 }`
  - `Numeric::from_str(&str) -> Result<Numeric, InvalidNumericError>` mirroring TS `parse`
  - `as_i64`, `as_u64`, `as_bigint`, `to_string`, `is_integer`
  - `equals`, `gt`, `lt`, `gte`, `lte` (consider implementing `PartialOrd`/`Ord` and `PartialEq`/`Eq` with care for canonical form)
  - Canonical form normalization so `Numeric("1.0")` and `Numeric("1")` compare equal (mirror TS behavior — verify with TS tests)
  - Unit tests including the precision cases below
- `crates/typespec-checker/src/types.rs` updates:
  - `NumericLiteralType { value: f64 }` → `NumericLiteralType { value: Numeric }`
  - `EnumMemberValue::Numeric(f64)` → `EnumMemberValue::Numeric(Numeric)`
- `crates/typespec-checker/src/numeric_ranges.rs` updates:
  - `NumericRange { min: f64, max: f64 }` → `NumericRange { min: Numeric, max: Numeric }`
  - `is_in_range`, `ranges_overlap`, `range_subsumes` use `Numeric` comparisons
  - Intrinsic range tables (int8…int64, uint8…uint64, safeint, float32/64, integer, numeric, decimal/decimal128) populated with `Numeric::from_str(...)` for boundary values that f64 can't represent
- `crates/typespec-checker/src/type_relation.rs` updates:
  - Literal narrowing uses `Numeric::lte`/`gte` instead of f64 comparison
  - String→literal parsing in tests goes through `Numeric::from_str` not `parse::<f64>()`
- `crates/typespec-checker/src/instantiation.rs` (M5e cache key):
  - If the memoization key embeds `f64` literal values, switch to `Numeric` so `Foo<9_223_372_036_854_775_807>` and `Foo<9_223_372_036_854_775_806>` produce different cache entries (today they collide because f64 rounds both to the same value)
- napi marshalling in `crates/typespec-napi/src/`:
  - Add `value_bigint: Option<BigInt>` for integer-valued numerics
  - Add `value_string: Option<String>` for non-integer numerics (decimal exponent representation as `"123.45e-2"` or `{n, e, d, s}` payload)
  - Keep `value_number: Option<f64>` as a convenience field for backwards compat — set only when the Numeric is `as_number().is_some()` (i.e., fits in f64 without loss)
  - JS-side test helper that reconstructs the Numeric from the BigInt/string fields for comparison
- New comparison tests that PROBE precision (these are the tests that would fail today but should pass with M7):
  - `int64` literal at boundary: `const x: int64 = 9223372036854775807;` — accept
  - `int64` literal one past: `const x: int64 = 9223372036854775808;` — diagnostic (out of range)
  - `uint64` literal at boundary
  - `safeint` boundary
  - Distinct `Foo<2^53>` vs `Foo<2^53 + 1>` instantiations get distinct cache entries
  - Decimal equality: `alias A = 0.3; alias B = 0.1; alias C = 0.2; ...` — Numeric equality semantics
  - Enum member with very large integer value
  - Range narrowing for decimal scalars

**Acceptance criteria**:

- All 822 existing tests still pass (this is a representation refactor, not a behavior change for in-range cases)
- New precision-probing tests (target ~10–15) pass on Rust and match TS exactly
- Zero remaining `f64` uses for literal value storage (search `crates/typespec-checker` and `crates/typespec-source` for `f64` and document any remaining justified uses; float scalars like `float32` may legitimately keep `f32`/`f64` representation for their _range bounds_ but the literal value type is `Numeric`)
- `Numeric` API parity with TS `Numeric` for the methods actually used by the checker

**Out of scope**:

- Performance optimization — BigInt arithmetic is 10–100× slower than f64; that's acceptable for a learning project
- Replacing scalar runtime representation in user-facing types (this is about literal _values_, not the storage of decorated numeric data)
- Custom decimal arithmetic (use `num-bigint` for the mantissa; expose only the operations the checker needs)

**Estimated complexity**: Medium (~300–500 LOC for `Numeric` itself + ~200–400 LOC of touches + ~50–100 LOC of napi marshalling + ~10–15 new tests)

---

### M8 — TS-shaped Program adapter for emitter compatibility

**Status when added**: The plan's "explicit non-goals" listed the emitter framework as out-of-scope. M8 partially walks that back: instead of porting the emitter framework to Rust, expose the Rust `check()` output as a **TS-shaped `Program` object** so existing JS-authored emitters can consume it via a thin adapter layer.

**Goal**: A JavaScript-side adapter (`packages/compiler-rust/program-adapter.js` or similar) that takes the napi `check()` output and presents it as a JS object graph matching the TS `Program` / `Type` interfaces closely enough that a typical navigator-style emitter Just Works. Validate against a small custom test emitter (not a real-world one — too much surface area).

**Reference**: see [ADR-0003](../adr/0003-ts-shaped-program-adapter.md).

**Key source files to mirror from**:

- `packages/compiler/src/core/types.ts` — `Program`, `Checker`, `Type` discriminated union (esp. Namespace, Model, ModelProperty, Scalar, Enum, EnumMember, Union, UnionVariant, Operation, Interface, Alias)
- `packages/compiler/src/core/semantic-walker.ts` — `navigateProgram(program, callbacks)` walker
- `packages/compiler/src/lib/intrinsic/decorators.ts` (M6 already handles state storage) — getters like `getDoc(program, target)`, `getSummary`, `getDeprecated`, `getKey`
- `packages/compiler/src/core/type-utils.ts` — small helpers emitters use frequently (`isErrorModel`, `getNamespaceFullName`, `isTemplateDeclaration`)

**Deliverables**:

- `packages/compiler-rust/src/program.ts` (or `.js` — TS for type safety, compiled if needed):
  - `createProgram(checkResult)` — takes napi `CheckResultJs`, returns a JS object graph matching TS `Program` shape
  - **Type identity is critical**: a single `Map<TypeId, JsTypeObject>` cache during materialization so `model.properties.get("x").model === model` holds and emitter `WeakMap<Type, ...>` caching works
  - Eager materialization for structural fields (kind, name, properties as a real `Map`, namespace pointer, etc.)
  - Decorator state side table exposed as `program._decoratorState: WeakMap<Type, Record<string, unknown>>` for the helpers to consult
- `packages/compiler-rust/src/walker.ts` — `navigateProgram(program, callbacks)` matching TS semantic-walker behavior (model, modelProperty, namespace, scalar, enum, enumMember, union, unionVariant, operation, interface, alias callbacks)
- `packages/compiler-rust/src/decorator-helpers.ts` — `getDoc(program, target)`, `getSummary`, `getDeprecated`, `getKey` (the 4 from M6) reading from the side table
- `packages/compiler-rust/src/type-utils.ts` — small set of helpers a navigator-style emitter typically needs: `isType`, `isTemplateDeclaration`, `getNamespaceFullName`, `isErrorModel` (if M6 added `@error` — likely not; document)
- `packages/compiler-rust/src/diagnostics.ts` — `Diagnostic` shape matching TS, `program.reportDiagnostic(d)` collecting into a list
- A **custom test emitter** at `packages/compiler-rust/test/emitter-compat/test-emitter.ts`:
  - Walks the program via `navigateProgram`
  - For each namespace/model/op, collects: kind, full name, doc (via `getDoc`), property list with types
  - Returns a structured JSON description of the spec
  - Reusable across Rust-adapter input and TS-Program input for comparison
- Comparison tests at `packages/compiler-rust/test/emitter-compat/*.test.ts`:
  - Run the test emitter against the Rust adapter
  - Run the same test emitter against the TS compiler's Program
  - Assert the output JSON matches structurally

**Test emitter validation cases** (target ~15–25 cases):

- Empty file
- Single model with primitive properties
- Nested models
- Model with `@doc`
- Enum with members + `@doc` on enum and a member
- Union with named variants
- Operation with parameters and return type
- Interface with multiple operations
- Nested namespaces + types
- Templated model instantiated at multiple sites — verify both instantiations show up
- Mutual model recursion
- Alias to a literal type
- `@deprecated` propagating through getDeprecated
- A model with `@@doc` augment decorator applied — both forms produce the same doc

**Acceptance criteria**:

- Test emitter produces structurally-identical JSON output when fed Rust-adapter-Program vs TS-Program for every case above
- `navigateProgram` callback invocation order matches TS for nested namespaces and inheritance chains
- Type identity holds across multiple property accesses (no duplicate JS objects for the same TypeId; verified via `===`)
- All 837 existing tests still pass
- `getDoc`/`getSummary`/`getDeprecated`/`getKey` return identical values to TS for every M6-decorated case

**Out of scope** (real emitters need these, but defer to M9+ if anyone ever wants them):

- Library / source-loading APIs (`program.checker.getGlobalNamespaceType` works; `program.host.readFile` doesn't)
- Source maps in emitFile / file-writing infrastructure
- Real-world emitters (JSON Schema, OpenAPI3) — the adapter targets navigator-style emitters; full Program interface fidelity is out of scope
- Mutable type graphs (TS emitters occasionally mutate; Rust adapter returns a frozen view)
- `Projection` / `templateMapper` / `getEffectiveModelType` / etc. — only the helpers the test emitter actually exercises
- Asset emitter framework (`@typespec/compiler/emitter-framework`)

**Estimated complexity**: Medium-Large (~600–1,200 LOC of JS/TS adapter code + walker + helpers + test emitter + 15–25 comparison tests; modest napi changes if the existing `check()` output already exposes enough state)

---

## Implementation Order

```
M1 (Scaffold) → M1.5 (Spike) → M2 (Scanner) → M2.5 (napi v3/ESM) → M3 (Parser) → M4 (Binder)
  → M5a (Models) → M5b (Enums/Unions) → M5c (Ops/Interfaces) → M5d (Relations) → M5e (Templates)
  → M6 (Decorators) → M7 (Numeric precision) → M8 (Program adapter)
```

Each milestone depends on the previous one. They cannot be parallelized.

## Agent Handoff Notes

When handing off to agents:

1. Each milestone is a self-contained task with clear inputs, outputs, and acceptance criteria
2. Agents should reference the TS source files listed in each milestone as the "specification"
3. Agents should run `cargo test` for Rust unit tests and `pnpm test` in `packages/compiler-rust` for comparison tests
4. The existing TS compiler must remain unchanged — this is purely additive
5. Agents must respect the cross-cutting constraints (SourceFile, threading model, position representation, error handling)
6. **At milestone boundaries**, evaluate against the kill criteria before starting the next one
7. Branch naming: prepend the author's username (e.g., `jeremymeng/rust-port-m2-scanner`)

## Open Questions (Deferred)

- Spread types — likely an extension of M5a or M5b, decide based on test failures
- Template instantiation memoization key design — explore during M5e
- Whether to expose the Rust scanner/parser to the TS compiler for performance (replacing the TS scanner with the Rust one) — explicitly out of scope, but conceivable future work
