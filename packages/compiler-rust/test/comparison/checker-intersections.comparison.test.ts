import { describe, expect, it } from "vitest";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  propertyIds: number[];
  propertyNames: string[];
  propertyTypeId?: number;
  returnTypeId?: number;
  targetId?: number;
  sourcePropertyId?: number;
  effectiveTypeId?: number;
}

interface RustCheckResult {
  types: RustType[];
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

function rust(source: string): RustCheckResult {
  return check(source) as RustCheckResult;
}

function errors(result: RustCheckResult) {
  return result.diagnostics.filter((x) => x.severity === "error");
}

function expectNoErrors(source: string): RustCheckResult {
  const result = rust(source);
  expect(errors(result)).toEqual([]);
  return result;
}

function typeById(result: RustCheckResult, id: number): RustType {
  return result.types[id];
}

function aliasTarget(result: RustCheckResult, name: string): RustType {
  const alias = result.types.find((x) => x.kind === "Alias" && x.name === name);
  expect(alias).toBeDefined();
  return typeById(result, alias!.targetId!);
}

function propertyType(result: RustCheckResult, propName: string): RustType {
  const prop = result.types.find((x) => x.kind === "ModelProperty" && x.name === propName);
  expect(prop).toBeDefined();
  return typeById(result, prop!.propertyTypeId!);
}

function operationReturnType(result: RustCheckResult, name: string): RustType {
  const op = result.types.find((x) => x.kind === "Operation" && x.name === name);
  expect(op).toBeDefined();
  return typeById(result, op!.returnTypeId!);
}

describe("checker intersections comparison: Rust", () => {
  const propertyCases = [
    ["two models", `model A { a: string; } model B { b: int32; } alias C = A & B;`, ["a", "b"]],
    ["empty right", `model A { a: string; } model B {} alias C = A & B;`, ["a"]],
    ["empty left", `model A {} model B { b: int32; } alias C = A & B;`, ["b"]],
    [
      "three models",
      `model A { a: string; } model B { b: int32; } model C0 { c: boolean; } alias C = A & B & C0;`,
      ["a", "b", "c"],
    ],
    [
      "nested left",
      `model A { a: string; } model B { b: int32; } model C0 { c: boolean; } alias C = (A & B) & C0;`,
      ["a", "b", "c"],
    ],
    [
      "nested right",
      `model A { a: string; } model B { b: int32; } model C0 { c: boolean; } alias C = A & (B & C0);`,
      ["a", "b", "c"],
    ],
    ["model expression", `model A { a: string; } alias C = A & { b: int32; };`, ["a", "b"]],
    ["two expressions", `alias C = { a: string; } & { b: int32; };`, ["a", "b"]],
    [
      "spread into expression",
      `model A { a: string; } model B { b: int32; } alias C = { ...A } & B;`,
      ["a", "b"],
    ],
    [
      "spread both",
      `model A { a: string; } model B { b: int32; } alias C = { ...A } & { ...B };`,
      ["a", "b"],
    ],
    [
      "inherited source",
      `model Base { base: string; } model A extends Base { a: string; } model B { b: int32; } alias C = A & B;`,
      ["a", "base", "b"],
    ],
    [
      "inherited both",
      `model BaseA { a0: string; } model A extends BaseA { a: string; } model BaseB { b0: string; } model B extends BaseB { b: int32; } alias C = A & B;`,
      ["a", "a0", "b", "b0"],
    ],
    ["literal properties", `model A { a: "x"; } model B { b: 1; } alias C = A & B;`, ["a", "b"]],
    [
      "union property",
      `model A { a: "x" | "y"; } model B { b: int32 | string; } alias C = A & B;`,
      ["a", "b"],
    ],
    [
      "array property",
      `model A { a: string[]; } model B { b: int32; } alias C = A & B;`,
      ["a", "b"],
    ],
    [
      "operation return",
      `model A { a: string; } model B { b: int32; } op read(): A & B;`,
      ["a", "b"],
    ],
    [
      "model property",
      `model A { a: string; } model B { b: int32; } model Holder { c: A & B; }`,
      ["a", "b"],
    ],
    [
      "alias operand",
      `model A { a: string; } alias AA = A; model B { b: int32; } alias C = AA & B;`,
      ["a", "b"],
    ],
    [
      "alias expression operand",
      `model A { a: string; } alias AA = { ...A }; model B { b: int32; } alias C = AA & B;`,
      ["a", "b"],
    ],
    ["empty intersection expression", `alias C = {} & {};`, []],
    [
      "four way",
      `model A { a: string; } model B { b: int32; } model C0 { c: boolean; } model D { d: bytes; } alias C = A & B & C0 & D;`,
      ["a", "b", "c", "d"],
    ],
    [
      "five way with empty",
      `model A { a: string; } model B {} model C0 { c: boolean; } model D {} model E { e: bytes; } alias C = A & B & C0 & D & E;`,
      ["a", "c", "e"],
    ],
    [
      "preserves order one",
      `model A { a1: string; a2: string; } model B { b1: string; b2: string; } alias C = A & B;`,
      ["a1", "a2", "b1", "b2"],
    ],
    [
      "preserves order two",
      `model A { a: string; } model B { b: string; } model C0 { c: string; } alias C = C0 & A & B;`,
      ["c", "a", "b"],
    ],
    [
      "nested model property",
      `model A { a: { x: string; }; } model B { b: int32; } alias C = A & B;`,
      ["a", "b"],
    ],
  ] as const;

  it.each(propertyCases)("creates intersection model: %s", (_name, source, props) => {
    const result = expectNoErrors(source);
    const target = source.includes("alias C")
      ? aliasTarget(result, "C")
      : source.includes("op read")
        ? operationReturnType(result, "read")
        : propertyType(result, "c");
    expect(target.kind).toBe("Model");
    expect(target.propertyNames).toEqual([...props]);
  });

  const spreadCases = [
    [
      "spread A and B",
      `model A { a: string; } model B { b: string; } model X { ...A, ...B }`,
      "X",
      ["a", "b"],
    ],
    [
      "spread inherited",
      `model Base { base: string; } model A extends Base { a: string; } model X { ...A }`,
      "X",
      ["a", "base"],
    ],
    ["spread alias", `model A { a: string; } alias AA = A; model X { ...AA }`, "X", ["a"]],
    [
      "spread anonymous alias",
      `model A { a: string; } alias AA = { ...A }; model X { ...AA }`,
      "X",
      ["a"],
    ],
    ["spread empty", `model A {} model X { ...A }`, "X", []],
    ["spread plus own", `model A { a: string; } model X { ...A, x: int32; }`, "X", ["a", "x"]],
    ["own plus spread", `model A { a: string; } model X { x: int32; ...A }`, "X", ["x", "a"]],
    [
      "nested spread",
      `model A { a: string; } model B { ...A, b: string; } model X { ...B }`,
      "X",
      ["a", "b"],
    ],
    [
      "spread scalar properties",
      `model A { a: int8; b: uint16; c: float32; } model X { ...A }`,
      "X",
      ["a", "b", "c"],
    ],
    [
      "spread literals",
      `model A { a: "x"; b: 1; c: true; } model X { ...A }`,
      "X",
      ["a", "b", "c"],
    ],
  ] as const;

  it.each(spreadCases)(
    "handles spread/intersection inputs: %s",
    (_name, source, modelName, props) => {
      const result = expectNoErrors(source);
      const model = result.types.find((x) => x.kind === "Model" && x.name === modelName)!;
      expect(model.propertyNames).toEqual([...props]);
    },
  );

  const diagnosticCases = [
    [
      "duplicate property",
      `model A { x: string; } model B { x: int32; } alias C = A & B;`,
      "intersect-duplicate-property",
    ],
    [
      "duplicate inherited property",
      `model Base { x: string; } model A extends Base {} model B { x: int32; } alias C = A & B;`,
      "intersect-duplicate-property",
    ],
    ["non-model right", `model A { x: string; } alias C = A & string;`, "intersect-non-model"],
    ["non-model left", `model A { x: string; } alias C = string & A;`, "intersect-non-model"],
    [
      "union operand",
      `model A { x: string; } alias C = A & (string | int32);`,
      "intersect-non-model",
    ],
    ["array operand", `model A { x: string; } alias C = A & string[];`, "intersect-invalid-index"],
    [
      "duplicate in nested",
      `model A { x: string; } model B { y: string; } model C0 { x: int32; } alias C = (A & B) & C0;`,
      "intersect-duplicate-property",
    ],
    [
      "duplicate in spread",
      `model A { x: string; } model B { x: int32; } alias C = { ...A } & { ...B };`,
      "intersect-duplicate-property",
    ],
    ["two non-models", `alias C = string & int32;`, "intersect-non-model"],
    ["literal non-model", `alias C = "x" & {};`, "intersect-non-model"],
    ["numeric non-model", `alias C = 1 & {};`, "intersect-non-model"],
    ["boolean non-model", `alias C = true & {};`, "intersect-non-model"],
    ["enum non-model", `enum E { a } alias C = E & {};`, "intersect-non-model"],
    [
      "union declaration non-model",
      `union U { a: string } alias C = U & {};`,
      "intersect-non-model",
    ],
    ["operation non-model", `op a(): void; alias C = a & {};`, "intersect-non-model"],
  ] as const;

  it.each(diagnosticCases)("reports intersection diagnostic: %s", (_name, source, code) => {
    expect(rust(source).diagnostics.map((x) => x.code)).toContain(code);
  });
});
