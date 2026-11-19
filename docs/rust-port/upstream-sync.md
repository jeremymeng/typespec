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
| `64f7850fb` Union `extends` clause (#11771)                | Parser `UnionStatement.extends`; binder/name-resolver; checker `UnionType.base_type`, `union-extends-disabled`, `extend-union`, `circular-base-type` (including union-expression self references), and variant assignability (`unassignable`) against the base type; napi + adapter `baseType`.                                                                                                                                                 |
| `8e16f8cff` Declarations as expressions (#11019)           | `Model/Scalar/Union/EnumDeclarationExpression` syntax kinds and AST nodes; parsing in primary expressions and expression lists; binding/resolution in node scope; checker `expression` flag on Model/Scalar/Enum (not registered in namespaces), `declaration-expression-disabled`, `templated-declaration-in-expression`; napi + adapter `expression`. Union declaration expressions are no longer flattened into enclosing union expressions. |
| `FunctionTypeExpression` syntax kind renumbering           | `SyntaxKind` numbering matches TS (`FunctionTypeExpression = 72`, declaration expressions `73–76`).                                                                                                                                                                                                                                                                                                                                             |
| `0bd0c1742` Recursive type assignability stack overflow    | Type relation guards source-union (cycle resolves as related) and target-union (cycle resolves as unrelated) pairs in the in-progress relation set, matching the existing model guard.                                                                                                                                                                                                                                                          |

Comparison coverage was added in `parser.comparison.test.ts`, `checker-unions.comparison.test.ts`, and
`checker-models.comparison.test.ts`.

### Known behavioral differences

- **Compiler features.** TS gates union `extends` and declaration expressions behind the
  `union-extends` / `declaration-expressions` features (enabled per library by `655c74139`). The Rust
  port has no feature configuration yet, so it always reports `union-extends-disabled` and
  `declaration-expression-disabled` — matching TS with features disabled (the default for user code).
  Revisit with M10/M13 when library loading and `tspconfig.yaml` exist.
- **Template instantiation of unions with `extends`.** Variants are validated against the base type on
  the template declaration only; TS also validates instantiated variants.

### Deferred (outside current scope or to be picked up by a later milestone)

- `4f8e31935` auto-decorator semantics (`auto-decorator-disabled`, `declarationKind`, modifier
  validation) — M10/M11.
- `8574d5453` typeref values (`TypeRefValue`, `getTypeRefType`, value kind) — M11/M12.
- `49aab186c` disallow property/record spreads in array model declarations (`no-array-properties`).
- `isDeclarationExpressionSym` handling for augment-decorator targets — M11.
- Re-checking enums under a template mapper.
- `9c9055062` preserve decorators on concrete types referenced from templates.
- `a06a62d03` suppress deprecated-type warnings inside deprecated operations.
- `5a18007e6` honor `@encodedName` on enum members — M11.
- `108ef0689` `Sym.node` / `getSymNode` typing (TS-only typing fix).
- `57f06f3db` `$provideTypeInfo` / `program.getTypeInfo` — M12.
- `325c16580` resolve `using` declared before the file namespace from the global namespace — M9
  (requires multi-file sources).
- `e453a4752` enum member navigation in linter rules.
- `16caf10f0` enter template scope before resolving operation parameter defaults.
- `ad69727db` ICE fix for interpolating a function call in a template declaration.
- `85ff8283c` `@encode(string)` on boolean — M11.
- `7aa17edfd` short diagnostic / linter rule names; `283ddec0c` unused suppressions; `463b08d58`
  duplicate `#suppress` warnings — diagnostics infrastructure, M13.
- Linter (`6f38b48f3`, `e218d1688`), CLI (`b3a9dc54c`, `c8105ea27`), `skipTrivia` export
  (`99eacfbd3`), server, formatter, and JSDoc-only changes — outside the checker port.
