import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  namespaceId?: number;
  operationIds: number[];
  operationNames: string[];
  sourceInterfaceIds: number[];
  parametersTypeId?: number;
  returnTypeId?: number;
  interfaceId?: number;
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
function iface(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Interface", name);
}
function operation(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.operationNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, owner.operationIds[index]);
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

describe("checker interfaces comparison: Rust vs TS", () => {
  const operationCases = [
    ["empty", `interface Service {}` as const, []],
    ["one op", `interface Service { list(): string; }`, ["list"]],
    ["two ops", `interface Service { list(): string; get(): int32; }`, ["list", "get"]],
    ["void op", `interface Service { ping(): void; }`, ["ping"]],
    ["params op", `interface Service { update(id: string, age: int32): void; }`, ["update"]],
    ["optional param", `interface Service { update(id?: string): void; }`, ["update"]],
    ["array return", `interface Service { list(): string[]; }`, ["list"]],
    ["union return", `interface Service { read(): string | int32; }`, ["read"]],
    ["model return", `model Pet {} interface Service { read(): Pet; }`, ["read"]],
    ["decorated op", `interface Service { @doc("x") list(): void; }`, ["list"]],
  ] as const;

  it.each(operationCases)("creates interface operations for %s", (_name, source, names) => {
    const result = expectNoErrors(source);
    const service = iface(result, "Service");
    expect(service.operationNames).toEqual(names);
    for (const name of names) {
      expect(operation(result, service, name).interfaceId).toBe(service.id);
    }
  });

  const returnCases = [
    ["string", "String"],
    ["int32", "Int32"],
    ["boolean", "Boolean"],
    ["void", "Void"],
    ["unknown", "Unknown"],
    ["bytes", "Bytes"],
    ["float32", "Float32"],
    ["utcDateTime", "UtcDateTime"],
    ["int64", "Int64"],
    ["uint32", "Uint32"],
    ["numeric", "Numeric"],
    ["integer", "Integer"],
    ["decimal", "Decimal"],
    ["decimal128", "Decimal128"],
    ["safeint", "SafeInt"],
    ["plainDate", "PlainDate"],
    ["plainTime", "PlainTime"],
    ["duration", "Duration"],
  ] as const;

  it.each(returnCases)("checks interface operation return %s", (returnType, expectedKind) => {
    const result = expectNoErrors(`interface Service { read(): ${returnType}; }`);
    expect(
      typeById(result, operation(result, iface(result, "Service"), "read").returnTypeId!).kind,
    ).toBe(expectedKind);
  });

  it("records decorators on interface and operation", () => {
    const result = expectNoErrors(
      `@doc("service") interface Service { @doc("list") list(): void; }`,
    );
    const service = iface(result, "Service");
    expect(service.decoratorNodeIds).toHaveLength(1);
    expect(operation(result, service, "list").decoratorNodeIds).toHaveLength(1);
  });

  it("extends one interface and copies operations", () => {
    const result = expectNoErrors(
      `interface Base { list(): string; } interface Service extends Base {}`,
    );
    const service = iface(result, "Service");
    expect(service.sourceInterfaceIds).toEqual([iface(result, "Base").id]);
    expect(service.operationNames).toEqual(["list"]);
    expect(operation(result, service, "list").interfaceId).toBe(service.id);
  });

  it("extends two interfaces in order", () => {
    const result = expectNoErrors(
      `interface A { a(): string; } interface B { b(): int32; } interface Service extends A, B {}`,
    );
    expect(iface(result, "Service").operationNames).toEqual(["a", "b"]);
  });

  it("allows own operation to override an extended operation", () => {
    const result = expectNoErrors(
      `interface Base { read(): string; } interface Service extends Base { read(): int32; }`,
    );
    const read = operation(result, iface(result, "Service"), "read");
    expect(typeById(result, read.returnTypeId!).kind).toBe("Int32");
  });

  it("wires namespace field for namespaced interface and operations", () => {
    const result = expectNoErrors(`namespace A { interface Service { list(): string; } }`);
    const namespace = namedType(result, "Namespace", "A");
    const service = iface(result, "Service");
    expect(service.namespaceId).toBe(namespace.id);
    expect(operation(result, service, "list").namespaceId).toBe(namespace.id);
  });

  it("supports op is from an interface operation", () => {
    const result = expectNoErrors(
      `interface Service { list(name: string): string; } op call is Service.list;`,
    );
    const call = result.types.find((x) => x.kind === "Operation" && x.name === "call")!;
    expect(call.returnTypeId).toBe(
      operation(result, iface(result, "Service"), "list").returnTypeId,
    );
  });

  const diagnosticCases = [
    [
      "duplicate own operation",
      `interface Service { list(): string; list(): int32; }`,
      ["interface-duplicate"],
    ],
    [
      "duplicate base operation",
      `interface A { x(): string; } interface B { x(): int32; } interface C extends A, B {}`,
      ["extends-interface-duplicate"],
    ],
    ["extends non-interface", `model M {} interface Service extends M {}`, ["extends-interface"]],
    ["unknown extended interface", `interface Service extends Missing {}`, ["invalid-ref"]],
    [
      "bad operation parameter",
      `interface Service { read(input: Missing): void; }`,
      ["invalid-ref"],
    ],
  ] as const;

  it.each(diagnosticCases)(
    "matches TS diagnostics for %s",
    async (_name, source, codes) => {
      await expectDiagnosticsLikeTs(source, [...codes]);
    },
    10000,
  );
});
