---
changeKind: internal
packages:
  - "@typespec/compiler"
---

Add a hidden, experimental `tsp compile --use-rust-checker` flag (or `TYPESPEC_USE_RUST_CHECKER=1`) that checks the program with the in-development Rust checker from `@typespec/compiler-rust` when it is available. Default behavior is unchanged.
