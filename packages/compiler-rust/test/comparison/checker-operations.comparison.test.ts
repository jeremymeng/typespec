import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  namespaceId?: number;
  propertyIds: number[];
  propertyNames: string[];
  propertyTypeId?: number;
  parametersTypeId?: number;
  returnTypeId?: number;
  sourceOperationId?: number;
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
function typeById(result: RustCheckResult, id: number): RustType {
  return result.types[id];
}
function namedType(result: RustCheckResult, kind: string, name: string): RustType {
  const found = result.types.find((x) => x.kind === kind && x.name === name);
  if (found === undefined) throw new Error(`Missing ${kind} ${name}`);
  return found;
}
function op(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Operation", name);
}
function paramModel(result: RustCheckResult, operation: RustType): RustType {
  return typeById(result, operation.parametersTypeId!);
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

describe("checker operations comparison: Rust vs TS", () => {
  const returnCases = [
    ["void", "Void"],
    ["string", "String"],
    ["int32", "Int32"],
    ["boolean", "Boolean"],
    ["unknown", "Unknown"],
    ["never", "Never"],
    ["bytes", "Bytes"],
    ["float64", "Float64"],
    ["plainDate", "PlainDate"],
    ["utcDateTime", "UtcDateTime"],
    ["Foo", "Model", "model Foo {}"],
    ["E", "Enum", "enum E { a }"],
    ["U", "Union", "union U { a: string }"],
    ["A", "Alias", "alias A = string;"],
    ['"ok"', "StringLiteral"],
    ["42", "NumericLiteral"],
    ["true", "BooleanLiteral"],
    ["false", "BooleanLiteral"],
    ["null", "Null"],
    ["numeric", "Numeric"],
    ["integer", "Integer"],
    ["safeint", "SafeInt"],
    ["decimal", "Decimal"],
    ["url", "Url"],
  ] as const;

  it.each(returnCases)(
    "creates operation returning %s",
    (returnType, expectedKind, prefix = "") => {
      const result = expectNoErrors(`${prefix} op read(): ${returnType};`);
      expect(typeById(result, op(result, "read").returnTypeId!).kind).toBe(expectedKind);
    },
  );

  const parameterCases = [
    ["no params", "op read(): void;", []],
    ["one string", "op read(name: string): void;", ["name"]],
    ["two params", "op read(name: string, age: int32): void;", ["name", "age"]],
    ["optional param", "op read(name?: string): void;", ["name"]],
    ["default param", 'op read(name?: string = "x"): void;', ["name"]],
    ["model param", "model Foo {} op read(input: Foo): void;", ["input"]],
    ["array param", "op read(values: string[]): void;", ["values"]],
    ["union param", "op read(value: string | int32): void;", ["value"]],
    [
      "spread param",
      "model Params { name: string; age: int32; } op read(...Params): void;",
      ["name", "age"],
    ],
    ["decorated param", '@doc("n") op read(@doc("p") name: string): void;', ["name"]],
  ] as const;

  it.each(parameterCases)("creates anonymous parameter model for %s", (_name, source, params) => {
    const result = expectNoErrors(source);
    const parameters = paramModel(result, op(result, "read"));
    expect(parameters.kind).toBe("Model");
    expect(parameters.name).toBe("");
    expect(parameters.propertyNames).toEqual(params);
  });

  it("wires op is source_operation and clones parameters", () => {
    const result = expectNoErrors(`op base(name: string): string; op read is base;`);
    const base = op(result, "base");
    const read = op(result, "read");
    expect(read.sourceOperationId).toBe(base.id);
    expect(read.parametersTypeId).not.toBe(base.parametersTypeId);
    expect(paramModel(result, read).propertyNames).toEqual(["name"]);
    expect(read.returnTypeId).toBe(base.returnTypeId);
  });

  it("copies base decorators before own decorators for op is", () => {
    const result = expectNoErrors(`@doc("base") op base(): void; @doc("read") op read is base;`);
    expect(op(result, "read").decoratorNodeIds).toHaveLength(2);
  });

  it("wires namespace field for namespaced operation and parameters", () => {
    const result = expectNoErrors(`namespace A { op read(name: string): void; }`);
    const namespace = namedType(result, "Namespace", "A");
    const read = op(result, "read");
    expect(read.namespaceId).toBe(namespace.id);
    expect(paramModel(result, read).namespaceId).toBe(namespace.id);
  });

  it("supports referencing an interface operation", () => {
    const result = expectNoErrors(`interface Service { list(): string; } op call is Service.list;`);
    expect(op(result, "call").sourceOperationId).toBeDefined();
    expect(typeById(result, op(result, "call").returnTypeId!).kind).toBe("String");
  });

  it("creates a void-return empty operation for invalid op is", () => {
    const result = rust(`model M {} op read is M;`);
    expect(errors(result).map((x) => x.code)).toContain("is-operation");
    expect(paramModel(result, op(result, "read")).propertyNames).toEqual([]);
  });

  const diagnosticCases = [
    ["op is non-operation", `model M {} op read is M;`, ["is-operation"]],
    ["self op is", `op read is read;`, ["circular-op-signature"]],
    ["mutual op is", `op a is b; op b is a;`, ["circular-op-signature"]],
    ["unknown parameter type", `op read(input: Missing): void;`, ["invalid-ref"]],
    ["duplicate top-level operation", `op read(): void; op read(): void;`, ["duplicate-symbol"]],
  ] as const;

  it.each(diagnosticCases)(
    "matches TS diagnostics for %s",
    async (_name, source, codes) => {
      await expectDiagnosticsLikeTs(source, [...codes]);
    },
    10000,
  );
});
