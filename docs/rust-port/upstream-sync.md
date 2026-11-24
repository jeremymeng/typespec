# Upstream Sync Log

This file tracks upstream `packages/compiler` changes merged into the Rust port and records
which ones were ported to the Rust crates and which were deferred.

## 2026-11 sync: `c93477f4a` → `upstream/main` (`b317c506c`)

Branch: `jeremymeng/rust-port-upstream-sync` (merged from `jeremymeng/rust-port-m8-program-adapter`).

### Ported (within M2–M8 scope)

| Upstream change                                            | Rust port                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `4f8e31935` Auto decorators (#10197) — syntax only         | Scanner `auto` keyword (`Token::AutoKeyword`, shifts later keyword ids) and `auto` modifier parsing; `SyntaxKind::AutoKeyword`.                                                                                                                                                                                                                                                                                                                 |
| `8574d5453` Experimental typeref values (#12461) — scanner | `typeref` is no longer a reserved keyword and parses as an identifier.                                                                                                                                                                                                                                                                                                                                                                          |
| `64f7850fb` Union `extends` clause (#11771)                | Parser `UnionStatement.extends`; binder/name-resolver; checker `UnionType.base_type`, `union-extends-disabled`, `extend-union`, `circular-base-type` (including union-expression self references), and variant assignability (`unassignable`) against the base type, re-validated per template instantiation (including templated bases); napi + adapter `baseType`.                                                                            |
| `8e16f8cff` Declarations as expressions (#11019)           | `Model/Scalar/Union/EnumDeclarationExpression` syntax kinds and AST nodes; parsing in primary expressions and expression lists; binding/resolution in node scope; checker `expression` flag on Model/Scalar/Enum (not registered in namespaces), `declaration-expression-disabled`, `templated-declaration-in-expression`; napi + adapter `expression`. Union declaration expressions are no longer flattened into enclosing union expressions. |
| `FunctionTypeExpression` syntax kind renumbering           | `SyntaxKind` numbering matches TS (`FunctionTypeExpression = 72`, declaration expressions `73–76`).                                                                                                                                                                                                                                                                                                                                             |
| `8e16f8cff` Re-checking enums under a template mapper      | Enum declaration expressions are re-created for each template instance (members owned by the instance enum); spread members resolve against the template arguments and `spread-enum` / `enum-member-duplicate` are reported per instance like TS.                                                                                                                                                                                               |
| `16caf10f0` Template scope for op parameter defaults       | Decorators are skipped on types checked in template declarations (incl. template parameter default instances and instances whose arguments reference template parameters); decorator arguments on instances resolve through the template mapper; a template parameter is assignable wherever its constraint is.                                                                                                                                 |
| `9c9055062` Decorators on concrete types from templates    | Concrete declarations (including enums and members reached via `Target.member`) are checked once outside any template context, so their decorators run exactly once regardless of whether they are first referenced from a template.                                                                                                                                                                                                            |
| `0bd0c1742` Recursive type assignability stack overflow    | Type relation guards source-union (cycle resolves as related) and target-union (cycle resolves as unrelated) pairs in the in-progress relation set, matching the existing model guard.                                                                                                                                                                                                                                                          |
| `49aab186c` No property/record spreads in array models     | `no-array-properties` for properties and property-introducing spreads in array models (own or inherited indexer); such spreads are reported once and skipped. `model X is T[]` is accepted as in TS (array expressions are anonymous in the port). Record spreads into array models (`model A is string[] { ...Record<string> }`) are reported once at the spread as of M10 (standard library loading).                                         |
| `325c16580` `using` before the file namespace (M9)         | Per-file `File` scopes hold file-level usings; a `using` declared before the file's blockless namespace resolves from the global namespace (covered by `multi-file.comparison.test.ts` and `crates/typespec-checker/tests/multi_file.rs`).                                                                                                                                                                                                      |

Comparison coverage was added in `parser.comparison.test.ts`, `checker-unions.comparison.test.ts`, and
`checker-models.comparison.test.ts`.

### Ported in M11 (standard decorators)

| Upstream change                                              | Rust port                                                                                                                                                                                                                                                                                                                          |
| ------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `4f8e31935` Auto decorators — semantics                      | `auto dec` declarations, `auto-decorator-disabled` (unless `CheckOptions.auto_decorators` / napi `autoDecorators`), modifier validation (`invalid-modifier`), `declarationKind`; auto implementations synthesized in `runDecorators`.                                                                                              |
| `isDeclarationExpressionSym` for augment-decorator targets   | `@@dec` targets that are declaration expressions / model or union expressions report `augment-decorator-target` like TS; an unresolved target reports only `invalid-ref`.                                                                                                                                                          |
| `5a18007e6` `@encodedName` on enum members                   | Covered by running the real `@typespec/compiler` implementation; enum member arguments/targets are marshalled (`std-decorators.comparison.test.ts`).                                                                                                                                                                               |
| `85ff8283c` `@encode(string)` on boolean                     | Covered by running the real implementation; `getEncode` compared against TS.                                                                                                                                                                                                                                                       |
| `a06a62d03` No deprecated warnings in deprecated operations  | Deprecation tracking added (`#deprecated` directives and deprecated decorator declarations, exposed through the `TypeSpec.deprecated` state map; `deprecated` / `duplicate-deprecation` / `invalid-deprecation-argument`); references in a deprecated context — including operations in a deprecated interface — are not reported. |
| Decorator argument / target checks, `missing-implementation` | Deferred from M10; see "Known behavioral differences" for the remaining value-system gaps.                                                                                                                                                                                                                                         |

### Handled in M12 (json-schema on the Program adapter)

- **Object / array value type-checking.** Object and array values get TS-shaped synthetic types
  (`createTypeForObjectValue` / `createTypeForArrayValue`, `crates/typespec-checker/src/checker/values.rs`);
  property defaults (including optional properties), typed `const` declarations and `valueof` decorator
  arguments (`@service(#{ title: 1 })` → `invalid-argument`) are checked with TS's flattened diagnostics
  (innermost literal property, `missing-property`, `unexpected-property`, first failing array element,
  combined messages otherwise). Compared with TS (codes, messages, positions) in
  `stdlib.comparison.test.ts`.
- **Tuple and string-template types.** `Tuple`, `StringTemplate` and `StringTemplateSpan` are modeled in
  Rust, napi and the adapter; tuple→array / tuple→tuple relations, `non-literal-string-template`,
  template substitution, and `Scopes extends string[] = []`-style defaults are checked against their
  constraint.
- **Default values.** `ModelProperty.defaultValue` is a TS `Value` (typed as the property type) on the
  adapter.
- **Checker parity found by the json-schema suite.** Model indexer relations (`missing-index`), qualified
  type names (`getTypeName`), union decorator ordering, missing-identifier `invalid-ref`, `T[]` counting as
  a template parameter use.

### Known behavioral differences

- **Compiler features (handled in M13).** `union-extends`, `declaration-expressions`, `auto-decorators` and
  `function-declarations` are enabled from `tspconfig.yaml` `features` and per library from the library's
  own `tspconfig.yaml` (TS `resolveLibraryFeatures`), via the napi `features` / `libraryFeatures` options
  used by `tsp compile --use-rust-checker`. Direct `checkProgram` callers still pass them explicitly.
- **Spread sources in template instances.** Instantiating `model Bar<U> { ...Foo<U> }` substitutes the
  copied properties directly instead of re-instantiating `Foo<A>`, so decorators run on `Bar<A>`'s
  properties (with resolved arguments) but `Foo<A>` is only created when referenced elsewhere.
- **Reserved keywords and modifiers as union variant names (parser).**
  - _Symptom:_ Rust reports `reserved-identifier` for union variants named with a reserved keyword
    (`self`, `this`, `super`, `with`, …) and `reserved-identifier` + `token-expected` for modifiers
    (`extern`, `internal`, `auto`); TS accepts both. Without a `:` (`union U { self }`), TS reports
    only `reserved-identifier`, while Rust also reports `invalid-ref` (keywords) or `token-expected`
    (modifiers). Model properties, enum members, operation parameters and model expression
    properties already match TS; only union variants differ.
  - _Repro:_ `union U { self: string }`, `union U { auto: string }`.
  - _Root cause:_ TS `parseUnionVariant` (`packages/compiler/src/core/parser.ts:805`) calls
    `parseIdOrValueForVariant` (`parser.ts:780`), which parses a reserved keyword or modifier token
    with `parseIdentifier({ allowReservedIdentifier: true })` (`parser.ts:785`) and only reports
    `reserved-identifier` when it is not followed by `:`. Rust `parse_union_variant`
    (`crates/typespec-parser/src/lib.rs:715`) calls `parse_expression()` directly, so the keyword
    reaches `parse_identifier` (`lib.rs:2114`) with `allow_reserved_identifier = false`.
  - _Fix:_ port `parseIdOrValueForVariant` into the Rust parser (keyword/modifier → identifier wrapped
    in a `TypeReference`, error only when no `:` follows) and add parser comparison cases. Owner: a
    parser-parity (M3) follow-up.
- **`unused-template-parameter` / `unused-using` linter gating (handled in M13).** Rust now reports both
  with their linter codes (`@typespec/compiler/unused-template-parameter`,
  `@typespec/compiler/unused-using`) in source order; the CLI bridge reports them only when the rule is
  enabled by the linter rule set (TS `createBuiltInLinter`) in the linting stage, which (like TS) is not
  reached once an error was reported. Direct `checkProgram` / `compile()` callers still get
  them unconditionally (filter by code). Remaining difference: with library rules also enabled, Rust
  built-in rule diagnostics are reported before all JS library rule diagnostics rather than interleaved
  by rule order. Owner: post-M13 linter follow-on.
- **Usings inside block namespaces (M9).** Rust stores `namespace A { using B; }` on the merged namespace
  scope, so such a using is visible in other `namespace A { }` blocks (including in other files); TS keeps
  it per namespace statement. File-level usings are per file and match TS.
- **Module resolution gaps (M9, narrowed in M13).** M13 added `tspconfig.yaml` `entrypoint` and
  `package.json#imports` to the JS host. Remaining: `#imports` only reads the nearest `package.json` and
  only supports `./` targets (no conditions objects, patterns, or package targets); no realpath/symlink
  canonicalization; the default simple `tspconfig.yaml` reader of `createNodeProgramHost` ignores
  `extends` (the CLI bridge plugs in the real YAML config loader). Owner: post-M13 module-resolution
  follow-on.
- **Decorator argument validation (M10 → M11).** Since M11, decorator arguments are checked against
  type and `valueof` parameters, targets (`decorator-wrong-target`) and `missing-implementation` like TS
  (`std-decorators.comparison.test.ts`). Remaining gaps are listed below under M11.
- **Adapter `checker.isValueOfType` for object / array values (M11, narrowed in M12).** Values are now
  type-checked by the Rust checker (see "Handled in M12"), but checks that JS decorator implementations
  run themselves still go through the adapter.
  - _Symptom:_ `@example(#{a: "x"}) model A { a: int32 }` — TS: `unassignable` "Type '"x"' is not
    assignable to type 'int32'" at `"x"`; Rust: `unassignable` "Type 'object value' is not assignable to
    type 'A'" at the object.
  - _Root cause:_ the adapter's `checker.isValueOfType` checks composite values structurally in JS and
    reports at the whole value; the napi relation export takes a type id and returns diagnostics without
    locations.
  - _Fix / owner:_ expose the Rust value check (`check_value_of_type`) through napi for decorator argument
    values (they already carry node ids). Owner: post-M13 emitter follow-on (`@typespec/openapi3`); no
    json-schema output depends on it.
- **Relation errors are not flattened to the innermost mismatch (M11).**
  - _Symptom:_ `op foo(x: string): void; @overload(foo) op bar(x: int32): void;` — TS: `unassignable`
    "Type 'int32' is not assignable to type 'string'" at `x: int32`; Rust (via the adapter's
    `isTypeAssignableTo`): `property-unassignable` "Types of property 'x' are incompatible …" at `bar`'s parameters.
  - _Root cause:_ TS relation diagnostics retarget to the deepest node that has a source location
    (`checker.ts` `isTypeAssignableTo` → `relation.ts` error chain flattening); Rust's relation keeps the
    top-level chain.
  - _Fix / owner:_ M12 ported the flattening for value checks only (`checker/values.rs`). Type relations
    requested through the adapter's `isTypeAssignableTo` still return the top-level chain without
    locations; port `relation.ts` error flattening into the napi relation export. Owner: post-M13 emitter
    follow-on.
- **Decorators run after checking (M11).** JS implementations are replayed over the finished adapter graph
  (`runDecorators`) instead of during `finishType`. Effects that TS decorators have on the _checker_ (e.g.
  mutating a type that a later declaration copies via `model is` / spread) are visible only on the adapter
  types they touch; copies made by the Rust checker do not see them. Types synthesized later
  (`checker.createType`/`finishType`, typekits, `filterModelProperties`) run their decorators and
  `onTargetFinish` validators but not `onGraphFinish`. Built-in compiler validators and `$onValidate` run
  after all `onGraphFinish` validators (TS order). M12 found no json-schema output depending on
  in-check mutation, so this is unchanged. Owner: post-M13 emitter follow-on, if an emitter depends on it.
- **Tuple `@minItems` / `@maxItems` (M12).** TS checks a tuple's length against the target array's
  `@minItems` / `@maxItems` in the relation (`isTupleAssignableToArrayModel`); Rust only checks each
  element. Repro: `@maxItems(1) model A is string[]; model M<T extends A> {} alias X = M<["a", "b"]>;`
  (TS: `invalid-argument`, Rust: none). Root cause: decorator state is not available to the Rust relation
  (decorators replay in JS). Owner: post-M13 (needs Rust-side `@minItems`/`@maxItems` state).
- **String templates with value interpolations (M12).** Templates whose spans reference `const` values
  (TS value-world string templates, `mixed-string-template`) are not modeled: spans are always types.
  Owner: post-M13 value-system follow-on.
- **Template value arguments are placeholders (M11).** `#{}` / `#[]` template arguments and defaults are an
  opaque `unknown` placeholder type in the Rust graph; they are turned into values only when used as
  decorator arguments, and their constraint is not checked (M12 did not change this; json-schema does not
  use template value arguments). Owner: post-M13 value-system follow-on.
- **`resolveTypeReference` (M11).** `program.resolveTypeReference` / `checker.resolveTypeReference` on the
  adapter resolve plain dotted paths only (no template arguments, no diagnostics). json-schema does not
  call it. Owner: post-M13 emitter follow-on (needs a napi entry point to check a reference expression).
- **Compiler features (M11, handled in M13).** `auto-decorators` / function-declaration gating now also
  follow `tspconfig.yaml` / library features when compiled through the CLI (see "Compiler features"
  above); the napi `autoDecorators` / `functionDeclarations` options remain for direct callers.
- **Intersections with several record indexers (M10).**
  - _Symptom:_ `alias I = Record<string> & Record<int32>` has no indexer in Rust; TS merges the value
    types (`indexer = { key: string, value: <merged string & int32> }`).
  - _Root cause:_ TS `checkIntersectionExpression` calls `mergeModelTypes` on the indexer values; Rust
    keeps an indexer only when exactly one option has one (`check_intersection` in `checker.rs`).
  - _Fix / owner:_ port the indexer-value merge (follow-up to M10; needed before M12 emitters see such types).
- **Single-source `check()` has no standard library (M10).** The napi `check(source)` API used by the
  M2–M8 comparison suites keeps the built-in intrinsic stubs (TS `nostdlib`-like): `Array`/`Record`,
  `TypeSpec` declarations and decorator declarations are only available through `checkProgram`. M12 kept
  it (the M2–M8 suites compare `nostdlib`-like behavior and their helpers pick types by name; converting
  32 suites is mechanical but large). New comparison suites use `checkProgram` / `compile`. M13 did not
  change it: the CLI goes through `checkProgram`, and converting the suites does not affect CLI parity.
  Owner: post-M13 test-infrastructure follow-on (switch `check()` to `checkProgram` with an in-memory
  host and convert the helpers).

### Handled in M13 (`tsp compile --use-rust-checker`)

- Diagnostics are printed by the TS CLI with `file:line:col` and code frames (real `SourceFile`s,
  identifier spans like TS `locateId`), with `#suppress`, `suppress-error`, `--warn-as-error` and exit codes.
- Linter gating of `unused-using` / `unused-template-parameter`, compiler features from `tspconfig.yaml`
  and libraries, `tspconfig.yaml` `entrypoint`, `package.json#imports` (see above).
- `7aa17edfd` short diagnostic / linter rule names: `#suppress` and linter rule codes resolve short names
  through the compiler's own `diagnostic-code` resolver; `ambiguous-short-name` is reported.
- `463b08d58` duplicate `#suppress`: `duplicate-suppression` warnings for project files.

### Known behavioral differences (M13 CLI bridge)

- **Diagnostic order across stages.** Rust returns checker diagnostics for the whole program before JS
  decorators run, so a decorator diagnostic on an earlier declaration prints after a checker diagnostic
  on a later one (TS interleaves them during checking). Name-resolution diagnostics (`invalid-ref`,
  `ambiguous-symbol`) are merged into the checker list by position (`merge_resolution_diagnostics` in
  `crates/typespec-napi/src/checker.rs`), which matches TS for the fixtures but is heuristic. Repro:
  `model A { @minValue(10) @maxValue(1) x: int32 } model B { x: Missing }` — TS prints `invalid-range`
  (1:11) then `invalid-ref` (2:14); Rust prints them reversed. Root cause: decorators replay after checking (M11).
  Owner: post-M13 (needs in-check decorator callbacks).
- **Not implemented in the bridge.** `--trace` output (tracer is a no-op), the compiler version mismatch
  check, `incompatible-library` (`validateLoadedLibraries`), template instantiation trace in diagnostic
  messages, codefixes, and `283ddec0c` unused suppressions (TS reports them only in the language server).
  `getSourceFileLocationContext` returns libraries without their `$lib` metadata name. Owner: post-M13
  CLI follow-on.
- **Single-shot only.** `tsp compile --watch --use-rust-checker` reuses the same path for each rebuild but
  is untested; `tsp` language server and `tsp init` do not use the Rust checker.

### Deferred (outside current scope or to be picked up by a later milestone)

- `8574d5453` typeref values (`TypeRefValue`, `getTypeRefType`, value kind) — not needed by json-schema;
  post-M13 value-system follow-on.
- `108ef0689` `Sym.node` / `getSymNode` typing (TS-only typing fix).
- `57f06f3db` `$provideTypeInfo` / `program.getTypeInfo` — not used by json-schema; post-M13 emitter
  follow-on (the adapter would call library `$provideTypeInfo` hooks after decorators).
- `@typespec/rest` decorator coverage (M11 stretch) — not needed by json-schema; `@typespec/openapi3`
  follow-on (out of M12 scope per the plan).
- `e453a4752` enum member navigation in linter rules.
- `ad69727db` ICE fix for interpolating a function call in a template declaration — blocked on
  `extern fn` / function calls (still not checked after M12; string templates are) / `Reflection`; the Rust checker
  does not panic on the upstream repro (covered by a unit test).
- `7aa17edfd` short names and `463b08d58` duplicate `#suppress` — handled in M13; `283ddec0c` unused
  suppressions — language-server only in TS, post-M13 CLI follow-on.
- Linter (`6f38b48f3`, `e218d1688`), CLI (`b3a9dc54c`, `c8105ea27`), `skipTrivia` export
  (`99eacfbd3`), server, formatter, and JSDoc-only changes — outside the checker port.
