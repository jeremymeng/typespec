import { strictEqual } from "node:assert";
import { describe, it } from "vitest";
import { hello } from "../index.js";

describe("compiler-rust native binding", () => {
  it("round-trips a hello string from Rust", () => {
    strictEqual(hello(), "hello from rust");
  });
});
