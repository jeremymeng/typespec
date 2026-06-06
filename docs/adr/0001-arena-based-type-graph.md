# Arena-based type graph for Rust compiler port

The Rust port of the TypeSpec compiler uses arena allocation with integer indices (`TypeId`, `NodeId`, `SymbolId`) instead of heap-allocated reference-counted pointers (`Rc<RefCell<T>>`) for the AST and type graph.

TypeSpec's type system creates cyclic graphs — models reference other models, unions contain types that reference back to the union, and templates instantiate types that reference the template. In TypeScript, this is handled trivially by garbage collection. In Rust, the three viable options were: (1) `Rc<RefCell<T>>` which simulates GC but is verbose and fights the borrow checker on every access, (2) raw pointers via `unsafe` which defeats the purpose of learning safe Rust, and (3) arena allocation where all types live in a `Vec<Type>` and references are plain `u32` indices. We chose arenas because this is the established pattern in Rust compilers (`rustc`, `rust-analyzer`) — it's safe, performant, and teaches idiomatic Rust ownership patterns. The cost is that indices are not type-checked at compile time (a `TypeId` could accidentally be used where a `NodeId` is expected), which we mitigate with newtype wrappers.

## Considered Options

- **`Rc<RefCell<T>>`** — Rejected. Every field access requires `.borrow()`, mutations require `.borrow_mut()`, and runtime panics replace compile-time guarantees. Verbose without teaching useful Rust patterns.
- **`unsafe` raw pointers** — Rejected. Defeats the learning goal of writing safe Rust. Would also make the code harder to reason about.
