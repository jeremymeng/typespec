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
| `49aab186c` No property/record spreads in array models     | `no-array-properties` for properties and property-introducing spreads in array models (own or inherited indexer); such spreads are reported once and skipped. `model X is T[]` is accepted as in TS (array expressions are anonymous in the port). Record spreads are not applicable: the port has no `Record<T>` (std library, M10).                                                                                                           |

Comparison coverage was added in `parser.comparison.test.ts`, `checker-unions.comparison.test.ts`, and
`checker-models.comparison.test.ts`.

### Known behavioral differences

- **Compiler features.** TS gates union `extends` and declaration expressions behind the
  `union-extends` / `declaration-expressions` features (enabled per library by `655c74139`). The Rust
  port has no feature configuration yet, so it always reports `union-extends-disabled` and
  `declaration-expression-disabled` — matching TS with features disabled (the default for user code).
  Revisit with M10/M13 when library loading and `tspconfig.yaml` exist.
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
- **`unused-template-parameter` is always reported (linter gating).**
  - _Symptom:_ Rust emits a `unused-template-parameter` warning for every unused template parameter;
    TS emits nothing by default.
  - _Repro:_ `model TargetA {} model Constrained<T extends TargetA> {}` (also `model M<T> {}`). `T`
    is genuinely unused: a constraint is not a usage in either implementation.
  - _Root cause:_ not a usage-tracking difference — usage marking matches TS for models, scalars,
    aliases, operations, unions, interfaces, defaults and self references (TS marks usage at
    `packages/compiler/src/core/checker.ts:1744` and `checker.ts:4146`; Rust at
    `crates/typespec-checker/src/checker.rs:1726`). In TS the check is a built-in linter rule
    (`packages/compiler/src/core/linter-rules/unused-template-parameter.rule.ts`, registered by
    `createBuiltInLinter` in `packages/compiler/src/core/linter.ts:587`) that only runs when enabled
    through the linter rule set (`packages/compiler/src/core/program.ts:452-522`); when enabled, TS
    reports the same parameters with code `@typespec/compiler/unused-template-parameter`. Rust
    reports it unconditionally from the checker (`check` → `report_unused_template_parameters`,
    `crates/typespec-checker/src/checker.rs:150`, `checker.rs:1730`) with the unqualified code
    `unused-template-parameter`, and iterates a `HashMap` so the report order is not stable.
  - _Fix:_ stop reporting from the checker; expose the usage map (like
    `getTemplateParameterUsageMap`) and run the rule in a linter stage gated by the linter rule set,
    reporting `@typespec/compiler/unused-template-parameter` in source order. Owner: M13 (linter
    configuration / `tsp compile` integration).

### Deferred (outside current scope or to be picked up by a later milestone)

- `4f8e31935` auto-decorator semantics (`auto-decorator-disabled`, `declarationKind`, modifier
  validation) — M10/M11.
- `8574d5453` typeref values (`TypeRefValue`, `getTypeRefType`, value kind) — M11/M12.
- `isDeclarationExpressionSym` handling for augment-decorator targets — M11.
- `a06a62d03` suppress deprecated-type warnings inside deprecated operations — blocked: the Rust
  checker does not implement deprecation tracking (`#deprecated` directive, `@deprecated`
  semantics) or the `deprecated` diagnostic yet — M11/M13.
- `5a18007e6` honor `@encodedName` on enum members — M11.
- `108ef0689` `Sym.node` / `getSymNode` typing (TS-only typing fix).
- `57f06f3db` `$provideTypeInfo` / `program.getTypeInfo` — M12.
- `325c16580` resolve `using` declared before the file namespace from the global namespace — M9
  (requires multi-file sources).
- `e453a4752` enum member navigation in linter rules.
- `ad69727db` ICE fix for interpolating a function call in a template declaration — blocked on
  `extern fn` / function calls / string template values / `Reflection` (M10–M12); the Rust checker
  does not panic on the upstream repro (covered by a unit test).
- `85ff8283c` `@encode(string)` on boolean — M11.
- `7aa17edfd` short diagnostic / linter rule names; `283ddec0c` unused suppressions; `463b08d58`
  duplicate `#suppress` warnings — diagnostics infrastructure, M13.
- Linter (`6f38b48f3`, `e218d1688`), CLI (`b3a9dc54c`, `c8105ea27`), `skipTrivia` export
  (`99eacfbd3`), server, formatter, and JSDoc-only changes — outside the checker port.
