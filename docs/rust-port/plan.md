# Plan: Port TypeSpec Compiler to Rust (Phase 1+2)

## Goal

Port the TypeSpec compiler's scanner, parser, binder, and checker from TypeScript to Rust as a learning project. The Rust implementation lives side-by-side with the existing TS compiler in the monorepo, connected via napi-rs bindings, validated by comparison tests and dual-runner tests.

> **Note on plan location**: This plan is committed to the repo at `docs/rust-port/plan.md` so it can be referenced by handoff agents (cloud, other contributors, future-you). The ADR for arena allocation lives at `docs/adr/0001-arena-based-type-graph.md`.

## Resolved Decisions

| Decision | Choice | Alternatives Rejected |
|----------|--------|-----------------------|
| Primary goal | Learning Rust | Production migration, performance |
| Location | In monorepo under `crates/` + `packages/compiler-rust` | Standalone repo |
| Scope | Scanner + Parser + Binder + Checker | Scanner/Parser only; full compiler including emitters |
| AST/type representation | Arena allocator with index-based references (`bumpalo`) | `Rc<RefCell<T>>`, raw pointers |
| napi-rs API surface | Fine-grained per-phase (`scan`, `parse`, `check`) | Coarse `compile()` only |
| Decorator handling | JS callbacks from Rust via napi-rs | Skip decorators; reimplement in Rust |
| Test strategy | Comparison tests + dual-runner where tests aren't TS-specific | Rewrite tests in Rust; comparison only |
| Stdlib/intrinsics | Hard-code intrinsics in Rust, parse stdlib `.tsp` files | Load everything from JS |
| Scanner approach | Hand-written (matching TS design) | Use `logos` crate |
| `SourceFile` abstraction | Replicate in Rust — scanner owns line/character mapping | Pass raw strings, reconstruct positions on JS side |
| napi-rs threading | Synchronous on main JS thread — `check()` is invoked from JS and stays there; decorator callbacks are direct `JsFunction` calls (not `ThreadsafeFunction`) | Async checker with `ThreadsafeFunction` |
| CI / prebuilt binaries | Use `@napi-rs/cli`'s standard cross-platform build matrix; publish prebuilds per platform | Build from source on install; single-platform support |
| Key crates | `napi`, `napi-derive`, `napi-build`, `bumpalo`, `string-interner` | `typed-arena`, `logos`, `miette` |

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

### M3 — Parser Parity

**Goal**: Port the TypeSpec parser from TS to Rust. Expose `parse(source: string) → AST` via napi-rs. Define the arena-based AST.

**Key source files to port from**:
- `packages/compiler/src/core/parser.ts` (3,085 lines)
- `packages/compiler/src/core/types.ts` (AST node type definitions)

**Deliverables**:
- `crates/typespec-parser/src/ast.rs` — Arena-allocated AST node types
- `crates/typespec-parser/src/syntax_kind.rs` — `SyntaxKind` enum matching TS
- `crates/typespec-parser/src/lib.rs` — `Parser` struct, `parse()` function
- `crates/typespec-napi/src/parser.rs` — napi binding: `parse(source: String) → SerializedAST`
- Comparison tests + dual-runner tests

**Implementation guidance**:
- The TS parser is a recursive-descent parser. Mirror this design in Rust.
- AST nodes reference each other. Use `NodeId` (index into arena) instead of direct references.
- The TS `SyntaxKind` enum has ~130 variants — port them all.
- Parser error recovery: the TS parser recovers from errors by inserting missing tokens. Replicate this behavior so diagnostic output matches.
- Key AST node types to port: `TypeSpecScriptNode`, `ModelStatementNode`, `EnumStatementNode`, `UnionStatementNode`, `OperationStatementNode`, `InterfaceStatementNode`, `ScalarStatementNode`, `AliasStatementNode`, `NamespaceStatementNode`, and all expression nodes.
- The API shape (eager vs lazy serialization) is determined by M1.5's spike findings.

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

## Implementation Order

```
M1 (Scaffold) → M1.5 (Spike) → M2 (Scanner) → M3 (Parser) → M4 (Binder)
  → M5a (Models) → M5b (Enums/Unions) → M5c (Ops/Interfaces) → M5d (Relations) → M5e (Templates)
  → M6 (Decorators)
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
