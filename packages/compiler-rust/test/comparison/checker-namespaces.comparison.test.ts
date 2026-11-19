import { describe, expect, it } from "vitest";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  namespaceId?: number;
  modelIds: number[];
  modelNames: string[];
  scalarIds: number[];
  scalarNames: string[];
  enumIds: number[];
  enumNames: string[];
  unionIds: number[];
  unionNames: string[];
  aliasIds: number[];
  aliasNames: string[];
  operationIds: number[];
  operationNames: string[];
  interfaceIds: number[];
  interfaceNames: string[];
  namespaceIds: number[];
  namespaceNames: string[];
  decoratorNodeIds: number[];
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
function namedType(result: RustCheckResult, kind: string, name: string): RustType {
  const found = result.types.find((x) => x.kind === kind && x.name === name);
  if (found === undefined) throw new Error(`Missing ${kind} ${name}`);
  return found;
}
function namespace(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Namespace", name);
}
function globalNamespace(result: RustCheckResult): RustType {
  return result.types.find((x) => x.kind === "Namespace" && x.name === "")!;
}

describe("checker namespaces comparison: Rust vs TS", () => {
  it("creates global namespace", () => {
    const result = expectNoErrors(`model Foo {}`);
    const global = globalNamespace(result);
    expect(global.namespaceId).toBeUndefined();
    expect(global.modelNames).toContain("Foo");
  });

  const memberCases = [
    ["model", `namespace A { model Foo {} }`, "modelNames", "Foo", "Model"],
    ["scalar", `namespace A { scalar Foo extends string; }`, "scalarNames", "Foo", "Scalar"],
    ["enum", `namespace A { enum Foo { a } }`, "enumNames", "Foo", "Enum"],
    ["union", `namespace A { union Foo { a: string } }`, "unionNames", "Foo", "Union"],
    ["alias", `namespace A { alias Foo = string; }`, "aliasNames", "Foo", "Alias"],
    ["operation", `namespace A { op Foo(): void; }`, "operationNames", "Foo", "Operation"],
    [
      "interface",
      `namespace A { interface Foo { read(): string; } }`,
      "interfaceNames",
      "Foo",
      "Interface",
    ],
    [
      "nested namespace",
      `namespace A { namespace Foo { model Bar {} } }`,
      "namespaceNames",
      "Foo",
      "Namespace",
    ],
  ] as const;

  it.each(memberCases)(
    "registers namespace %s members",
    (_name, source, collection, member, kind) => {
      const result = expectNoErrors(source);
      const ns = namespace(result, "A");
      expect(ns[collection]).toContain(member);
      expect(namedType(result, kind, member).namespaceId).toBe(ns.id);
    },
  );

  const nestedCases = [
    [`namespace A.B { model Foo {} }`, ["A", "B"]],
    [`namespace A.B.C { model Foo {} }`, ["A", "B", "C"]],
    [`namespace A { namespace B { namespace C { model Foo {} } } }`, ["A", "B", "C"]],
  ] as const;

  it.each(nestedCases)("creates nested namespace chain %#", (source, names) => {
    const result = expectNoErrors(source);
    for (const name of names) expect(namespace(result, name)).toBeDefined();
    for (let i = 1; i < names.length; i++) {
      expect(namespace(result, names[i]).namespaceId).toBe(namespace(result, names[i - 1]).id);
    }
  });

  it("merges reopened block namespaces", () => {
    const result = expectNoErrors(`namespace A { model X {} } namespace A { model Y {} }`);
    expect(result.types.filter((x) => x.kind === "Namespace" && x.name === "A")).toHaveLength(1);
    expect(namespace(result, "A").modelNames).toEqual(["X", "Y"]);
  });

  it("merges semicolon and block namespace declarations", () => {
    const result = expectNoErrors(`namespace A; model X {} namespace B { model Y {} }`);
    expect(namespace(result, "A").modelNames).toEqual(["X"]);
    expect(namespace(result, "B").namespaceId).toBe(namespace(result, "A").id);
  });

  // TS rejects these orderings in the parser; the namespaces are still bound.
  const orderingCases = [
    [`namespace A { model X {} } namespace A; model Y {}`, ["blockless-namespace-first"]],
    [`namespace A { namespace B; model X {} }`, ["blockless-namespace-first"]],
    [`namespace A; namespace B; model Foo {}`, ["multiple-blockless-namespace"]],
    [
      `namespace A; namespace B; namespace C; model X {}`,
      ["multiple-blockless-namespace", "multiple-blockless-namespace"],
    ],
  ] as const;

  it.each(orderingCases)("reports namespace ordering error %#", (source, codes) => {
    expect(errors(rust(source)).map((x) => x.code)).toEqual(codes);
  });

  it("records namespace decorators", () => {
    const result = expectNoErrors(`@doc("a") namespace A {}`);
    expect(namespace(result, "A").decoratorNodeIds).toHaveLength(1);
  });

  it("wires top-level declarations to the global namespace", () => {
    const result = expectNoErrors(
      `model M {} scalar S extends string; enum E { a } union U { a: string } alias A = string; op O(): void; interface I {}`,
    );
    const global = globalNamespace(result);
    expect(global.modelNames).toContain("M");
    expect(global.scalarNames).toContain("S");
    expect(global.enumNames).toContain("E");
    expect(global.unionNames).toContain("U");
    expect(global.aliasNames).toContain("A");
    expect(global.operationNames).toContain("O");
    expect(global.interfaceNames).toContain("I");
  });

  it("resolves names from nested namespace by walking outward", () => {
    const result = expectNoErrors(`namespace A { model X {} namespace B { model Y { x: X; } } }`);
    expect(namespace(result, "B").modelNames).toEqual(["Y"]);
  });

  it("resolves nearest nested declaration first", () => {
    const result = expectNoErrors(
      `namespace A { model X {} namespace B { model X {} model Y { x: X; } } }`,
    );
    expect(result.types.filter((x) => x.kind === "Model" && x.name === "X")).toHaveLength(2);
  });

  const namespaceForms = [
    `namespace A; model X {}`,
    `namespace A { model X {} }`,
    `namespace A.B; model X {}`,
    `namespace A.B { model X {} }`,
    `namespace A { namespace B { model X {} } }`,
    `namespace A { op one(): void; op two(): void; }`,
    `namespace A { interface One {} interface Two {} }`,
    `namespace A { scalar One extends string; scalar Two extends string; }`,
    `namespace A { enum One { a } enum Two { b } }`,
    `namespace A { union One { a: string } union Two { b: int32 } }`,
    `namespace A { alias One = string; alias Two = int32; }`,
    `namespace A.B.C.D { model X {} }`,
    `namespace A { namespace B { namespace C { namespace D { model X {} } } } }`,
    `namespace A { @doc("b") namespace B {} }`,
  ] as const;

  it.each(namespaceForms)("accepts namespace form %#", (source) => {
    expect(errors(rust(source))).toEqual([]);
  });

  const collisionCases = [
    [`namespace A { model X {} model X {} }`, "duplicate-symbol"],
    [`namespace A { model X {} op X(): void; }`, "duplicate-symbol"],
    [`namespace A { interface X {} union X { a: string } }`, "duplicate-symbol"],
    [`namespace A { namespace X {} model X {} }`, "duplicate-symbol"],
  ] as const;

  it.each(collisionCases)("reports namespace collision %#", (source, code) => {
    expect(rust(source).diagnostics.map((x) => x.code)).toContain(code);
  });
});
