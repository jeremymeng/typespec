import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  memberIds: number[];
  memberNames: string[];
  baseTypeId?: number;
  propertyTypeId?: number;
  variantTypeId?: number;
  decoratorNodeIds: number[];
  expression?: boolean;
  literalString?: string;
  literalNumeric?: number;
  literalBoolean?: boolean;
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

function namedType(result: RustCheckResult, kind: string, name: string): RustType {
  const found = result.types.find((x) => x.kind === kind && x.name === name);
  if (found === undefined) {
    throw new Error(
      `Missing ${kind} ${name}. Found: ${result.types.map((x) => `${x.kind}:${x.name}`)}`,
    );
  }
  return found;
}

function unionType(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Union", name);
}

function variant(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.memberNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, owner.memberIds[index]);
}

function property(result: RustCheckResult, name: string): RustType {
  const prop = result.types.find((x) => x.kind === "ModelProperty" && x.name === name);
  expect(prop).toBeDefined();
  return prop!;
}

async function expectDiagnosticsLikeTs(source: string, codes: string[]) {
  const rustDiagnostics = rust(source)
    .diagnostics.filter((x) => codes.includes(x.code))
    .map(({ code, message }) => ({ code, message }));
  const tsDiagnostics = (await Tester.diagnose(source))
    .filter((x) => codes.includes(x.code))
    .map(({ code, message }) => ({ code, message }));
  expect(rustDiagnostics).toEqual(tsDiagnostics);
}

describe("checker unions comparison: Rust vs TS", () => {
  const declaredCases = [
    [
      "two named variants",
      `union Result { ok: int32, err: string }`,
      "Result",
      ["ok", "err"],
      ["Int32", "String"],
    ],
    [
      "semicolon separators",
      `union U { a: string; b: int32; }`,
      "U",
      ["a", "b"],
      ["String", "Int32"],
    ],
    ["single variant", `union Single { value: string }`, "Single", ["value"], ["String"]],
    [
      "boolean variant",
      `union Booly { yes: boolean, no: string }`,
      "Booly",
      ["yes", "no"],
      ["Boolean", "String"],
    ],
    [
      "numeric scalars",
      `union Numbers { small: int8, large: int64 }`,
      "Numbers",
      ["small", "large"],
      ["Int8", "Int64"],
    ],
    [
      "model variants",
      `model Ok {} model Err {} union Result { ok: Ok, err: Err }`,
      "Result",
      ["ok", "err"],
      ["Model", "Model"],
    ],
    [
      "literal variants",
      `union Mode { read: "read", write: "write" }`,
      "Mode",
      ["read", "write"],
      ["StringLiteral", "StringLiteral"],
    ],
    [
      "numeric literal variants",
      `union Code { ok: 200, fail: 500 }`,
      "Code",
      ["ok", "fail"],
      ["NumericLiteral", "NumericLiteral"],
    ],
    [
      "boolean literal variants",
      `union Flag { yes: true, no: false }`,
      "Flag",
      ["yes", "no"],
      ["BooleanLiteral", "BooleanLiteral"],
    ],
    [
      "nested named union as variant",
      `union Inner { a: string } union Outer { inner: Inner, n: int32 }`,
      "Outer",
      ["inner", "n"],
      ["Union", "Int32"],
    ],
    [
      "mixed scalar and literal",
      `union Mixed { a: string, b: "x", c: 1 }`,
      "Mixed",
      ["a", "b", "c"],
      ["String", "StringLiteral", "NumericLiteral"],
    ],
    [
      "bytes or string",
      `union Data { data: bytes, text: string }`,
      "Data",
      ["data", "text"],
      ["Bytes", "String"],
    ],
    [
      "date scalars",
      `union Dates { date: plainDate, instant: utcDateTime }`,
      "Dates",
      ["date", "instant"],
      ["PlainDate", "UtcDateTime"],
    ],
    ["empty union", `union Empty {}`, "Empty", [], []],
    [
      "unnamed declared variants",
      `union UnnamedMix { string, int32, boolean }`,
      "UnnamedMix",
      ["Symbol(name)#0", "Symbol(name)#1", "Symbol(name)#2"],
      ["String", "Int32", "Boolean"],
    ],
  ] as const;

  it.each(declaredCases)("creates declared union: %s", (_name, source, unionName, names, kinds) => {
    const result = expectNoErrors(source);
    const un = unionType(result, unionName);
    expect(un.expression).toBe(false);
    expect(un.memberNames).toEqual([...names]);
    names.forEach((name, index) => {
      const v = variant(result, un, name);
      expect(v.kind).toBe("UnionVariant");
      expect(v.baseTypeId).toBe(un.id);
      expect(typeById(result, v.variantTypeId!).kind).toBe(kinds[index]);
    });
  });

  const expressionCases = [
    ["string or int32", `model Foo { x: string | int32; }`, ["String", "Int32"]],
    ["three options", `model Foo { x: string | int32 | boolean; }`, ["String", "Int32", "Boolean"]],
    ["literal options", `model Foo { x: "read" | "write"; }`, ["StringLiteral", "StringLiteral"]],
    [
      "numeric literals",
      `model Foo { x: 1 | 2 | 3; }`,
      ["NumericLiteral", "NumericLiteral", "NumericLiteral"],
    ],
    ["boolean literals", `model Foo { x: true | false; }`, ["BooleanLiteral", "BooleanLiteral"]],
    ["model options", `model A {} model B {} model Foo { x: A | B; }`, ["Model", "Model"]],
    [
      "flattens left",
      `model Foo { x: (string | int32) | boolean; }`,
      ["String", "Int32", "Boolean"],
    ],
    [
      "flattens right",
      `model Foo { x: string | (int32 | boolean); }`,
      ["String", "Int32", "Boolean"],
    ],
    ["skips never", `model Foo { x: string | never; }`, ["String"]],
    [
      "keeps named union option",
      `union Named { a: string } model Foo { x: Named | int32; }`,
      ["Union", "Int32"],
    ],
    ["float scalars", `model Foo { x: float32 | float64; }`, ["Float32", "Float64"]],
    ["bytes or string", `model Foo { x: bytes | string; }`, ["Bytes", "String"]],
    ["unknown or string", `model Foo { x: unknown | string; }`, ["Unknown", "String"]],
    ["decimal or integer", `model Foo { x: decimal | integer; }`, ["Decimal", "Integer"]],
    [
      "mixed literal kinds",
      `model Foo { x: "a" | 1 | true; }`,
      ["StringLiteral", "NumericLiteral", "BooleanLiteral"],
    ],
    ["void or string", `model Foo { x: void | string; }`, ["Void", "String"]],
  ] as const;

  it.each(expressionCases)("creates anonymous union expression: %s", (_name, source, kinds) => {
    const result = expectNoErrors(source);
    const propType = typeById(result, property(result, "x").propertyTypeId!);
    expect(propType.kind).toBe("Union");
    expect(propType.expression).toBe(true);
    expect(propType.memberNames).toHaveLength(kinds.length);
    expect(propType.memberNames.every((x, i) => x === `Symbol(name)#${i}`)).toBe(true);
    expect(
      propType.memberIds.map((id) => typeById(result, typeById(result, id).variantTypeId!).kind),
    ).toEqual([...kinds]);
  });

  it("records decorators on declared union and variants", () => {
    const result = expectNoErrors(
      `@doc("result") union Result { @doc("ok") ok: int32, err: string }`,
    );
    const resultUnion = unionType(result, "Result");
    expect(resultUnion.decoratorNodeIds).toHaveLength(1);
    expect(variant(result, resultUnion, "ok").decoratorNodeIds).toHaveLength(1);
    expect(variant(result, resultUnion, "err").decoratorNodeIds).toHaveLength(0);
  });

  it("supports omitted variant names with synthetic symbol names", () => {
    const result = expectNoErrors(`union Mode { "read", "write", int32 }`);
    const mode = unionType(result, "Mode");
    expect(mode.memberNames).toEqual(["Symbol(name)#0", "Symbol(name)#1", "Symbol(name)#2"]);
  });

  it("resolves model property type to a named union", () => {
    const result = expectNoErrors(
      `union Mode { read: "read", write: "write" } model Foo { mode: Mode; }`,
    );
    expect(property(result, "mode").propertyTypeId).toBe(unionType(result, "Mode").id);
  });

  it("canonicalizes repeated literal union variants", () => {
    const result = expectNoErrors(
      `union Mode { a: "same", b: "same", c: 1, d: 1, e: true, f: true }`,
    );
    expect(
      result.types.filter((x) => x.kind === "StringLiteral" && x.literalString === "same"),
    ).toHaveLength(1);
    expect(
      result.types.filter((x) => x.kind === "NumericLiteral" && x.literalNumeric === 1),
    ).toHaveLength(1);
    expect(
      result.types.filter((x) => x.kind === "BooleanLiteral" && x.literalBoolean === true),
    ).toHaveLength(1);
  });

  it("reports duplicate named variants like TS", async () => {
    await expectDiagnosticsLikeTs(`union U { a: string, a: int32 }`, ["union-duplicate"]);
  }, 10000);

  it("does not report duplicates for omitted variant names", () => {
    expectNoErrors(`union U { string, string }`);
  });

  describe("union extends", () => {
    const diagnosticCodes = [
      "union-extends-disabled",
      "extend-union",
      "unassignable",
      "circular-base-type",
    ];

    it.each([
      ["assignable variants", `union U extends string { a: "a", b: "b" }`],
      ["unassignable variant", `union U extends string { a: "a", b: 1 }`],
      ["model expression base", `union U extends { a: string } { a: { a: "x" } }`],
      ["operation base", `op o(): void; union U extends o { a: string }`],
      ["self reference", `union U extends U { a: string }`],
      ["union expression self reference", `union U extends U | string { a: string }`],
      ["named union base", `union B { string, int32 } union U extends B { a: string }`],
      ["templated base", `union U<T> extends T { a: T } alias X = U<string>;`],
      ["unknown base", `union U extends unknown { a: string }`],
      ["unresolved base", `union U extends Missing { a: string }`],
      ["int32 base", `union U extends int32 { a: 1, b: "x" }`],
    ])(
      "reports diagnostics like TS: %s",
      async (_name, source) => {
        await expectDiagnosticsLikeTs(source, diagnosticCodes);
      },
      10000,
    );

    it("records the base type", () => {
      const result = rust(`union U extends string { a: "a" }`);
      const u = unionType(result, "U");
      expect(typeById(result, u.baseTypeId!).kind).toBe("String");
      expect(u.memberNames).toEqual(["a"]);
    });
  });

  it("does not flatten union declaration expressions into union expressions", () => {
    const result = rust(`model Foo { x: union { a: string } | int32; }`);
    const propType = typeById(result, property(result, "x").propertyTypeId!);
    expect(propType.memberIds).toHaveLength(2);
    const nested = typeById(result, typeById(result, propType.memberIds[0]).variantTypeId!);
    expect(nested.kind).toBe("Union");
    expect(nested.expression).toBe(true);
    expect(nested.memberNames).toEqual(["a"]);
  });
});
