import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  nodeId?: number;
  symbolId?: number;
  memberIds: number[];
  memberNames: string[];
  baseTypeId?: number;
  propertyTypeId?: number;
  decoratorNodeIds: number[];
  literalString?: string;
  literalNumeric?: number;
  enumMemberValueString?: string;
  enumMemberValueNumeric?: number;
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

function enumType(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Enum", name);
}

function enumMember(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.memberNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, owner.memberIds[index]);
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

describe("checker enums comparison: Rust vs TS", () => {
  const declarationCases = [
    ["two bare members", `enum Status { Active, Inactive }`, ["Active", "Inactive"]],
    ["single member", `enum One { Only }`, ["Only"]],
    ["three members", `enum Direction { North, East, South }`, ["North", "East", "South"]],
    ["string-valued", `enum Code { Ok: "ok", Bad: "bad" }`, ["Ok", "Bad"]],
    ["numeric-valued", `enum Port { Http: 80, Https: 443 }`, ["Http", "Https"]],
    ["mixed-valued", `enum Mixed { A, B: "bee", C: 3 }`, ["A", "B", "C"]],
    [
      "quoted keyword values",
      `enum Keyword { String: "string", Model: "model" }`,
      ["String", "Model"],
    ],
    ["zero numeric", `enum Zero { None: 0, One: 1 }`, ["None", "One"]],
    ["decimal numeric", `enum Fraction { Half: 0.5, Quarter: 0.25 }`, ["Half", "Quarter"]],
    [
      "pascal names",
      `enum HttpStatus { Ok, NotFound, BadRequest }`,
      ["Ok", "NotFound", "BadRequest"],
    ],
    ["camel names", `enum State { pending, running, done }`, ["pending", "running", "done"]],
    ["empty enum", `enum Empty {}`, []],
    [
      "mixed string and bare",
      `enum Sizes { Small: "S", Medium, Large: "L" }`,
      ["Small", "Medium", "Large"],
    ],
    ["mixed numeric and bare", `enum Digits { Zero: 0, One, Two: 2 }`, ["Zero", "One", "Two"]],
    [
      "http numeric values",
      `enum Http { Continue: 100, Ok: 200, Redirect: 302, Error: 500 }`,
      ["Continue", "Ok", "Redirect", "Error"],
    ],
    ["letter string values", `enum Letters { A: "A", B: "B", C: "C" }`, ["A", "B", "C"]],
    ["four lower names", `enum Lower { a, b, c, d }`, ["a", "b", "c", "d"]],
    ["five members", `enum Many { A, B, C, D, E }`, ["A", "B", "C", "D", "E"]],
    ["numeric suffix names", `enum NumericNames { One1, Two2 }`, ["One1", "Two2"]],
    ["underscore name", `enum Underscore { Foo_Bar }`, ["Foo_Bar"]],
  ] as const;

  it.each(declarationCases)("creates enum: %s", (_name, source, members) => {
    const result = expectNoErrors(source);
    const en = result.types.find((x) => x.kind === "Enum")!;
    expect(en.memberNames).toEqual([...members]);
    expect(en.memberIds).toHaveLength(members.length);
    for (const memberName of members) {
      const member = enumMember(result, en, memberName);
      expect(member.kind).toBe("EnumMember");
      expect(member.baseTypeId).toBe(en.id);
    }
  });

  const valueCases = [
    [`enum E { A: "a" }`, "A", "a", undefined],
    [`enum E { A: "" }`, "A", "", undefined],
    [`enum E { A: "with space" }`, "A", "with space", undefined],
    [`enum E { A: 0 }`, "A", undefined, 0],
    [`enum E { A: 1 }`, "A", undefined, 1],
    [`enum E { A: 42 }`, "A", undefined, 42],
    [`enum E { A: 3.14 }`, "A", undefined, 3.14],
    [`enum E { A, B: "b", C: 2 }`, "A", undefined, undefined],
    [`enum E { A: "dash-value" }`, "A", "dash-value", undefined],
    [`enum E { A: "UPPER" }`, "A", "UPPER", undefined],
    [`enum E { A: 1000 }`, "A", undefined, 1000],
    [`enum E { A: 0.001 }`, "A", undefined, 0.001],
    [`enum E { A, B: "b", C: 2 }`, "B", "b", undefined],
    [`enum E { A, B: "b", C: 2 }`, "C", undefined, 2],
  ] as const;

  it.each(valueCases)(
    "records enum member value %#",
    (source, memberName, stringValue, numericValue) => {
      const result = expectNoErrors(source);
      const member = enumMember(result, enumType(result, "E"), memberName);
      expect(member.enumMemberValueString).toBe(stringValue);
      expect(member.enumMemberValueNumeric).toBe(numericValue);
    },
  );

  it("does not synthesize implicit enum numeric values", () => {
    const result = expectNoErrors(`enum E { A, B }`);
    expect(enumMember(result, enumType(result, "E"), "A").enumMemberValueNumeric).toBeUndefined();
    expect(enumMember(result, enumType(result, "E"), "B").enumMemberValueNumeric).toBeUndefined();
  });

  it("records enum decorators without executing them", () => {
    const result = expectNoErrors(`@doc("status") enum Status { @doc("active") Active, Inactive }`);
    const status = enumType(result, "Status");
    expect(status.decoratorNodeIds).toHaveLength(1);
    expect(enumMember(result, status, "Active").decoratorNodeIds).toHaveLength(1);
    expect(enumMember(result, status, "Inactive").decoratorNodeIds).toHaveLength(0);
  });

  it("allows model properties to reference enum declarations", () => {
    const result = expectNoErrors(`enum Status { Active, Inactive } model Foo { status: Status; }`);
    const status = enumType(result, "Status");
    const prop = result.types.find((x) => x.kind === "ModelProperty" && x.name === "status")!;
    expect(prop.propertyTypeId).toBe(status.id);
  });

  it("clones spread enum members into the target enum", () => {
    const result = expectNoErrors(`enum Base { One: "one", Two } enum Spread { ...Base, Three }`);
    const spread = enumType(result, "Spread");
    expect(spread.memberNames).toEqual(["One", "Two", "Three"]);
    expect(enumMember(result, spread, "One").baseTypeId).toBe(spread.id);
    expect(enumMember(result, spread, "One").enumMemberValueString).toBe("one");
  });

  it("reports duplicate enum members like TS", async () => {
    await expectDiagnosticsLikeTs(`enum E { A, A }`, ["enum-member-duplicate"]);
  }, 15000);

  it("reports duplicate enum members introduced by spread like TS", async () => {
    await expectDiagnosticsLikeTs(`enum Base { A } enum E { A, ...Base }`, [
      "enum-member-duplicate",
    ]);
  });

  it("reports spreading non-enum like TS", async () => {
    await expectDiagnosticsLikeTs(`model NotEnum {} enum E { ...NotEnum }`, ["spread-enum"]);
  });

  it("canonicalizes repeated string enum values", () => {
    const result = expectNoErrors(`enum E { A: "same", B: "same" }`);
    const literals = result.types.filter(
      (x) => x.kind === "StringLiteral" && x.literalString === "same",
    );
    expect(literals).toHaveLength(1);
  });

  it("canonicalizes repeated numeric enum values", () => {
    const result = expectNoErrors(`enum E { A: 42, B: 42 }`);
    const literals = result.types.filter(
      (x) => x.kind === "NumericLiteral" && x.literalNumeric === 42,
    );
    expect(literals).toHaveLength(1);
  });
});
