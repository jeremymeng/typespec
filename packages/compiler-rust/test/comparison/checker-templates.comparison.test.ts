import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  propertyIds: number[];
  propertyNames: string[];
  indexerTypeId?: number;
  propertyTypeId?: number;
  memberIds: number[];
  memberNames: string[];
  operationIds: number[];
  operationNames: string[];
  parametersTypeId?: number;
  returnTypeId?: number;
  targetId?: number;
  variantTypeId?: number;
  templateParameterIds: number[];
  templateArgumentIds: number[];
  isTemplateDeclaration: boolean;
  constraintId?: number;
  templateDefaultId?: number;
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

function property(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.propertyNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, owner.propertyIds[index]);
}

function propertyTarget(result: RustCheckResult, owner: RustType, name: string): RustType {
  return typeById(result, property(result, owner, name).propertyTypeId!);
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

describe("checker templates comparison: Rust vs TS", () => {
  const declarationCases = [
    ["simple model", `model Foo<T> { value: T; }`, "Model", "Foo", 1],
    ["constrained model", `model Foo<T extends string> { value: T; }`, "Model", "Foo", 1],
    ["defaulted model", `model Foo<T = int32> { value: T; }`, "Model", "Foo", 1],
    ["default references earlier", `model Foo<T, U = T> { a: T; b: U; }`, "Model", "Foo", 2],
    ["pair model", `model Pair<A, B> { left: A; right: B; }`, "Model", "Pair", 2],
    ["array model", `model Box<T> { values: T[]; }`, "Model", "Box", 1],
    ["nested model expression", `model Box<T> { value: { inner: T }; }`, "Model", "Box", 1],
    ["scalar template", `scalar MyScalar<T extends string>;`, "Scalar", "MyScalar", 1],
    ["union template", `union Result<T, E> { ok: T, err: E }`, "Union", "Result", 2],
    ["interface template", `interface Service<T> { get(): T; }`, "Interface", "Service", 1],
    ["operation template", `op create<T>(input: T): T;`, "Operation", "create", 1],
    ["alias template", `alias Wrapped<T> = { value: T };`, "Alias", "Wrapped", 1],
  ] as const;

  it.each(declarationCases)(
    "records template declaration metadata for %s",
    (_name, source, kind, name, parameterCount) => {
      const result = expectNoErrors(source);
      const type = namedType(result, kind, name);
      expect(type.isTemplateDeclaration).toBe(true);
      expect(type.templateParameterIds).toHaveLength(parameterCount);
      for (const id of type.templateParameterIds) {
        expect(typeById(result, id).kind).toBe("TemplateParameter");
      }
    },
  );

  const instantiationCases = [
    ["model string", `model Foo<T> { value: T; } model X { y: Foo<string>; }`, "y", "String"],
    ["model int32", `model Foo<T> { value: T; } model X { y: Foo<int32>; }`, "y", "Int32"],
    ["model boolean", `model Foo<T> { value: T; } model X { y: Foo<boolean>; }`, "y", "Boolean"],
    [
      "model literal string",
      `model Foo<T> { value: T; } model X { y: Foo<"hi">; }`,
      "y",
      "StringLiteral",
    ],
    [
      "model literal numeric",
      `model Foo<T> { value: T; } model X { y: Foo<42>; }`,
      "y",
      "NumericLiteral",
    ],
    [
      "model literal boolean",
      `model Foo<T> { value: T; } model X { y: Foo<true>; }`,
      "y",
      "BooleanLiteral",
    ],
    ["default string", `model Foo<T = string> { value: T; } model X { y: Foo; }`, "y", "String"],
    ["default int32", `model Foo<T = int32> { value: T; } model X { y: Foo; }`, "y", "Int32"],
    ["default bool", `model Foo<T = boolean> { value: T; } model X { y: Foo; }`, "y", "Boolean"],
    [
      "second default",
      `model Foo<T, U = int32> { value: U; } model X { y: Foo<string>; }`,
      "y",
      "Int32",
    ],
    [
      "default references earlier",
      `model Foo<T, U = T> { value: U; } model X { y: Foo<string>; }`,
      "y",
      "String",
    ],
    [
      "two explicit args",
      `model Foo<T, U> { value: U; } model X { y: Foo<string, int32>; }`,
      "y",
      "Int32",
    ],
    [
      "array substitution",
      `model Foo<T> { values: T[]; } model X { y: Foo<string>; }`,
      "y",
      "Model",
    ],
    [
      "nested model substitution",
      `model Foo<T> { value: { inner: T }; } model X { y: Foo<string>; }`,
      "y",
      "Model",
    ],
  ] as const;

  it.each(instantiationCases)("instantiates %s", (_name, source, propertyName, expectedKind) => {
    const result = expectNoErrors(source);
    const x = namedType(result, "Model", "X");
    const instantiated = propertyTarget(result, x, propertyName);
    expect(instantiated.templateArgumentIds.length).toBeGreaterThanOrEqual(1);
    const value = instantiated.propertyNames.includes("value")
      ? propertyTarget(result, instantiated, "value")
      : propertyTarget(result, instantiated, "values");
    expect(value.kind).toBe(expectedKind);
  });

  it("memoizes identical instantiations in one model", () => {
    const result = expectNoErrors(
      `model Foo<T> { value: T; } model X { a: Foo<string>; b: Foo<string>; }`,
    );
    const x = namedType(result, "Model", "X");
    expect(propertyTarget(result, x, "a").id).toBe(propertyTarget(result, x, "b").id);
  });

  it("memoizes identical instantiations across models", () => {
    const result = expectNoErrors(
      `model Foo<T> { value: T; } model X { a: Foo<string>; } model Z { b: Foo<string>; }`,
    );
    expect(propertyTarget(result, namedType(result, "Model", "X"), "a").id).toBe(
      propertyTarget(result, namedType(result, "Model", "Z"), "b").id,
    );
  });

  it("creates different types for different arguments", () => {
    const result = expectNoErrors(
      `model Foo<T> { value: T; } model X { a: Foo<string>; b: Foo<int32>; }`,
    );
    const x = namedType(result, "Model", "X");
    expect(propertyTarget(result, x, "a").id).not.toBe(propertyTarget(result, x, "b").id);
  });

  const kindCases = [
    [
      "alias model target",
      `alias Wrapped<T> = { value: T }; model X { y: Wrapped<string>; }`,
      "Alias",
    ],
    [
      "union variants",
      `union Result<T, E> { ok: T, err: E } model X { y: Result<string, int32>; }`,
      "Union",
    ],
    [
      "interface operation",
      `interface Service<T> { get(): T; } model X { y: Service<string>; }`,
      "Interface",
    ],
    ["scalar", `scalar Tagged<T extends string>; model X { y: Tagged<string>; }`, "Scalar"],
  ] as const;

  it.each(kindCases)("instantiates %s", (_name, source, expectedKind) => {
    const result = expectNoErrors(source);
    const y = propertyTarget(result, namedType(result, "Model", "X"), "y");
    expect(y.kind).toBe(expectedKind);
    expect(y.templateArgumentIds).toHaveLength(expectedKind === "Union" ? 2 : 1);
  });

  it("substitutes alias template targets", () => {
    const result = expectNoErrors(
      `alias Wrapped<T> = { value: T }; model X { y: Wrapped<string>; }`,
    );
    const y = propertyTarget(result, namedType(result, "Model", "X"), "y");
    const target = typeById(result, y.targetId!);
    expect(propertyTarget(result, target, "value").kind).toBe("String");
  });

  it("substitutes union variants", () => {
    const result = expectNoErrors(
      `union Result<T, E> { ok: T, err: E } model X { y: Result<string, int32>; }`,
    );
    const y = propertyTarget(result, namedType(result, "Model", "X"), "y");
    expect(y.memberNames).toEqual(["ok", "err"]);
    expect(typeById(result, typeById(result, y.memberIds[0]).variantTypeId!).kind).toBe("String");
    expect(typeById(result, typeById(result, y.memberIds[1]).variantTypeId!).kind).toBe("Int32");
  });

  it("substitutes interface operation return types", () => {
    const result = expectNoErrors(
      `interface Service<T> { get(): T; } model X { y: Service<string>; }`,
    );
    const y = propertyTarget(result, namedType(result, "Model", "X"), "y");
    const get = typeById(result, y.operationIds[y.operationNames.indexOf("get")]);
    expect(typeById(result, get.returnTypeId!).kind).toBe("String");
  });

  it("substitutes operation templates in signature references", () => {
    const result = expectNoErrors(`op create<T>(input: T): T; op createString is create<string>;`);
    const op = namedType(result, "Operation", "createString");
    expect(typeById(result, op.returnTypeId!).kind).toBe("String");
    const params = typeById(result, op.parametersTypeId!);
    expect(propertyTarget(result, params, "input").kind).toBe("String");
  });

  it("handles self-recursive model templates through the instantiation cache", () => {
    const result = expectNoErrors(
      `model Tree<T> { value: T; children: Tree<T>[]; } model X { t: Tree<string>; }`,
    );
    const tree = propertyTarget(result, namedType(result, "Model", "X"), "t");
    const children = propertyTarget(result, tree, "children");
    expect(typeById(result, children.indexerTypeId!).id).toBe(tree.id);
  });

  it("handles mutual model template recursion", () => {
    const result = expectNoErrors(
      `model A<T> { b: B<T>; } model B<T> { a: A<T>; } model X { a: A<string>; }`,
    );
    const a = propertyTarget(result, namedType(result, "Model", "X"), "a");
    const b = propertyTarget(result, a, "b");
    expect(propertyTarget(result, b, "a").id).toBe(a.id);
  });

  const tsDiagnosticCases = [
    [
      "too many arguments",
      `model Foo<T> { value: T; } model X { y: Foo<string, int32>; }`,
      ["invalid-template-args"],
    ],
    [
      "missing required argument",
      `model Foo<T> { value: T; } model X { y: Foo; }`,
      ["invalid-template-args"],
    ],
    [
      "non-template arguments",
      `model Foo { value: string; } model X { y: Foo<string>; }`,
      ["invalid-template-args"],
    ],
    [
      "default references later parameter",
      `model Foo<T = U, U = string> { value: T; }`,
      ["invalid-template-default"],
    ],
  ] as const;

  it.each(tsDiagnosticCases)(
    "matches TS diagnostics for %s",
    async (_name, source, codes) => {
      await expectDiagnosticsLikeTs(source, [...codes]);
    },
    10_000,
  );

  const argumentKindCases = [
    ["string", "string", "String"],
    ["int8", "int8", "Int8"],
    ["int16", "int16", "Int16"],
    ["int32", "int32", "Int32"],
    ["int64", "int64", "Int64"],
    ["uint8", "uint8", "Uint8"],
    ["uint16", "uint16", "Uint16"],
    ["uint32", "uint32", "Uint32"],
    ["boolean", "boolean", "Boolean"],
    ["literal", `"x"`, "StringLiteral"],
  ] as const;

  it.each(argumentKindCases)(
    "records %s as the canonical template argument",
    (_name, arg, kind) => {
      const result = expectNoErrors(`model Foo<T> { value: T; } model X { y: Foo<${arg}>; }`);
      const y = propertyTarget(result, namedType(result, "Model", "X"), "y");
      expect(typeById(result, y.templateArgumentIds[0]).kind).toBe(kind);
    },
  );

  const defaultKindCases = [
    ["string", "string", "String"],
    ["bytes", "bytes", "Bytes"],
    ["boolean", "boolean", "Boolean"],
    ["int32", "int32", "Int32"],
    ["safeint", "safeint", "SafeInt"],
    ["float32", "float32", "Float32"],
    ["decimal", "decimal", "Decimal"],
    ["plainDate", "plainDate", "PlainDate"],
    ["unknown", "unknown", "Unknown"],
    ["literal", `"x"`, "StringLiteral"],
  ] as const;

  it.each(defaultKindCases)("applies %s defaults before memoization", (_name, value, kind) => {
    const result = expectNoErrors(
      `model Foo<T = ${value}> { value: T; } model X { a: Foo; b: Foo<${value}>; }`,
    );
    const x = namedType(result, "Model", "X");
    const a = propertyTarget(result, x, "a");
    const b = propertyTarget(result, x, "b");
    expect(a.id).toBe(b.id);
    expect(typeById(result, a.templateArgumentIds[0]).kind).toBe(kind);
  });

  const constraintAcceptCases = [
    ["string accepts string", "string", "string"],
    ["string accepts literal", "string", `"x"`],
    ["numeric accepts int32", "numeric", "int32"],
    ["integer accepts int8", "integer", "int8"],
    ["boolean accepts boolean", "boolean", "boolean"],
    ["unknown accepts model", "unknown", "{ value: string }"],
  ] as const;

  it.each(constraintAcceptCases)("accepts constraint case %s", (_name, constraint, arg) => {
    expectNoErrors(`model Foo<T extends ${constraint}> { value: T; } model X { y: Foo<${arg}>; }`);
  });

  const referenceCases = [
    ["model property", `model Foo<T> { value: T; } model X { y: Foo<string>; }`],
    ["model spread", `model Foo<T> { value: T; } model X { ...Foo<string> }`],
    ["model is", `model Foo<T> { value: T; } model X is Foo<string>;`],
    ["model extends", `model Foo<T> { value: T; } model X extends Foo<string> {}`],
    ["interface extends", `interface Foo<T> { get(): T; } interface X extends Foo<string> {}`],
    ["union reference", `union Foo<T> { value: T } alias X = Foo<string>;`],
  ] as const;

  it.each(referenceCases)("supports template references in %s", (_name, source) => {
    expectNoErrors(source);
  });

  const unusedCases = [
    ["model", `model Foo<T> { value: string; }`],
    ["scalar", `scalar Foo<T>;`],
    ["union", `union Foo<T> { value: string }`],
    ["interface", `interface Foo<T> { get(): string; }`],
    ["operation", `op foo<T>(): string;`],
    ["alias", `alias Foo<T> = string;`],
  ] as const;

  it.each(unusedCases)("reports unused template parameter on %s", (_name, source) => {
    const result = rust(source);
    expect(result.diagnostics).toContainEqual(
      expect.objectContaining({ severity: "warning", code: "unused-template-parameter" }),
    );
  });

  it("reports constraint violations", () => {
    const result = rust(`model Foo<T extends string> { value: T; } model X { y: Foo<boolean>; }`);
    expect(errors(result).map((x) => x.code)).toContain("unassignable");
  });

  it("reports default constraint violations", () => {
    const result = rust(`model Foo<T extends string = int32> { value: T; }`);
    expect(errors(result).map((x) => x.code)).toContain("unassignable");
  });

  it("reports unused template parameters as warnings", () => {
    const result = rust(`model Foo<T> { value: string; }`);
    expect(result.diagnostics).toContainEqual(
      expect.objectContaining({ severity: "warning", code: "unused-template-parameter" }),
    );
  });
});
