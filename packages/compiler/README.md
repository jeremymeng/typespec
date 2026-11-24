# TypeSpec compiler and CLI

This package implements the core of the [TypeSpec](https://github.com/microsoft/typespec)
compiler and its command-line interface.

## Experimental: Rust checker

`tsp compile --use-rust-checker` (or `TYPESPEC_USE_RUST_CHECKER=1`) type-checks the program with the
in-development Rust port of the checker (`@typespec/compiler-rust`, not published) and runs emitters on the
result. It is experimental, hidden from `--help`, and only available in a development checkout of this
repository; see [docs/rust-port/README.md](../../docs/rust-port/README.md). Default behavior is unchanged.

## See also

- [TypeSpec Getting Started](https://github.com/microsoft/typespec#getting-started)
- [TypeSpec Website](https://typespec.io)
