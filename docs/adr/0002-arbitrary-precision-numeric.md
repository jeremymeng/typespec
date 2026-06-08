# Arbitrary-precision Numeric for literal values

The Rust port replaces `f64` literal value storage with a TypeSpec-faithful `Numeric` struct (`BigInt` mantissa + decimal exponent + sign + decimal-digit count), mirroring TS `core/numeric.ts`.

M5b chose `f64` as a pragmatic simplification when the type checker was first introduced. That choice left latent precision bugs: `int64`/`uint64` literals near their boundaries silently round, decimal arithmetic produces binary-approximation artifacts (`0.1 + 0.2 != 0.3`), and template instantiation memoization collisions occur for distinct large literals. The current comparison suite doesn't probe these cases, so no test actually fails — but the divergence from TS is real and would surface as soon as a user wrote a spec with large integer literals or precise decimals. We considered three alternatives: keep `f64` (rejected — diverges from TS), use `rust_decimal::Decimal` (rejected — capped at 96 bits, still loses for very large or very precise values), or mirror TS's structure exactly. We chose the mirror approach because the cost is bounded (~500–1000 LOC including tests and napi marshalling), `num-bigint` provides the arbitrary-precision integer, napi v3 supports `BigInt` FFI natively, and it gives byte-for-byte parity with the TS reference. The performance penalty (10–100× vs f64) is irrelevant for a learning-project compiler.

## Considered Options

- **Keep `f64`** — Rejected. Latent precision bugs at int64/uint64 boundaries and for any decimal value, plus template instantiation cache collisions that would silently merge distinct instantiations.
- **`rust_decimal::Decimal` (96-bit fixed-precision)** — Rejected. Better than f64 but caps at ~28 significant digits, so `decimal128` and the very-large-integer cases still lose. Not worth the half-measure.
- **Composite enum (`Numeric { Small(i64), Big(BigInt), Float(f64), BigDecimal(BigDecimal) }`)** — Rejected. Branch-by-branch optimization adds complexity without solving the canonical-equality problem (when does `Small(1)` equal `BigDecimal(1.0)`?) cleanly.

## Consequences

- Adds `num-bigint` dependency to `typespec-source` (and transitively to all downstream crates)
- `Numeric` equality must use canonical form (`Numeric("1.0") == Numeric("1")`) — mirror TS behavior here exactly to avoid breaking M5e's instantiation cache
- napi marshalling becomes two-path: `BigInt` for integer-valued numerics, structured payload for non-integers — JS consumers need a small reconstruction helper
- M5e template instantiation memoization keys change; verify the cache still hits the way TS does
