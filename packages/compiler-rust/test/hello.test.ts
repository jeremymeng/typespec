import { strictEqual } from "node:assert";
import { createRequire } from "node:module";
import { describe, it } from "vitest";

const require = createRequire(import.meta.url);
const native = require("../index.js") as { hello(): string };

describe("compiler-rust native binding", () => {
  it("round-trips a hello string from Rust", () => {
    strictEqual(native.hello(), "hello from rust");
  });
});
