import { describe, expect, it } from "vitest";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  memberIds: number[];
  memberNames: string[];
  propertyIds: number[];
  propertyNames: string[];
  baseTypeId?: number;
  propertyTypeId?: number;
  targetId?: number;
  variantTypeId?: number;
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

function aliasType(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Alias", name);
}

function property(result: RustCheckResult, name: string): RustType {
  const prop = result.types.find((x) => x.kind === "ModelProperty" && x.name === name);
  expect(prop).toBeDefined();
  return prop!;
}

function nonAlias(result: RustCheckResult, type: RustType): RustType {
  const seen = new Set<number>();
  let current = type;
  while (current.kind === "Alias" && current.targetId !== undefined && !seen.has(current.id)) {
    seen.add(current.id);
    current = typeById(result, current.targetId);
  }
  return current;
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

describe("checker aliases comparison: Rust vs TS", () => {
  const simpleCases = [
    ["string", `alias A = string;`, "A", "String"],
    ["int32", `alias A = int32;`, "A", "Int32"],
    ["boolean", `alias A = boolean;`, "A", "Boolean"],
    ["bytes", `alias A = bytes;`, "A", "Bytes"],
    ["unknown", `alias A = unknown;`, "A", "Unknown"],
    ["null", `alias A = null;`, "A", "Null"],
    ["string literal", `alias A = "hello";`, "A", "StringLiteral"],
    ["numeric literal", `alias A = 42;`, "A", "NumericLiteral"],
    ["boolean true literal", `alias A = true;`, "A", "BooleanLiteral"],
    ["boolean false literal", `alias A = false;`, "A", "BooleanLiteral"],
    ["model", `model Foo {} alias A = Foo;`, "A", "Model"],
    ["enum", `enum Status { Active } alias A = Status;`, "A", "Enum"],
    ["union", `union Result { ok: string } alias A = Result;`, "A", "Union"],
    ["float64", `alias A = float64;`, "A", "Float64"],
    ["decimal", `alias A = decimal;`, "A", "Decimal"],
    ["plainDate", `alias A = plainDate;`, "A", "PlainDate"],
    ["void", `alias A = void;`, "A", "Void"],
    ["never", `alias A = never;`, "A", "Never"],
  ] as const;

  it.each(simpleCases)("creates alias to %s", (_name, source, aliasName, targetKind) => {
    const result = expectNoErrors(source);
    const alias = aliasType(result, aliasName);
    expect(alias.targetId).toBeDefined();
    expect(typeById(result, alias.targetId!).kind).toBe(targetKind);
  });

  const unionAliasCases = [
    ["string-int", `alias Mode = string | int32;`, ["String", "Int32"]],
    ["literals", `alias Mode = "read" | "write";`, ["StringLiteral", "StringLiteral"]],
    ["numbers", `alias Mode = 1 | 2;`, ["NumericLiteral", "NumericLiteral"]],
    ["booleans", `alias Mode = true | false;`, ["BooleanLiteral", "BooleanLiteral"]],
    ["models", `model A {} model B {} alias Mode = A | B;`, ["Model", "Model"]],
    ["with never", `alias Mode = string | never;`, ["String"]],
    ["nested", `alias Mode = (string | int32) | boolean;`, ["String", "Int32", "Boolean"]],
    [
      "named union plus scalar",
      `union Named { a: string } alias Mode = Named | int32;`,
      ["Union", "Int32"],
    ],
    ["float scalars", `alias Mode = float32 | float64;`, ["Float32", "Float64"]],
    ["bytes or string", `alias Mode = bytes | string;`, ["Bytes", "String"]],
    ["unknown or string", `alias Mode = unknown | string;`, ["Unknown", "String"]],
    ["decimal or integer", `alias Mode = decimal | integer;`, ["Decimal", "Integer"]],
    [
      "mixed literal kinds",
      `alias Mode = "a" | 1 | true;`,
      ["StringLiteral", "NumericLiteral", "BooleanLiteral"],
    ],
  ] as const;

  it.each(unionAliasCases)(
    "creates alias to union expression: %s",
    (_name, source, targetKinds) => {
      const result = expectNoErrors(source);
      const target = typeById(result, aliasType(result, "Mode").targetId!);
      expect(target.kind).toBe("Union");
      expect(
        target.memberIds.map((id) => typeById(result, typeById(result, id).variantTypeId!).kind),
      ).toEqual([...targetKinds]);
    },
  );

  it("aliases can chain and getNonAlias reaches the final target", () => {
    const result = expectNoErrors(`alias A = string; alias B = A; alias C = B;`);
    expect(nonAlias(result, aliasType(result, "A")).kind).toBe("String");
    expect(nonAlias(result, aliasType(result, "B")).kind).toBe("String");
    expect(nonAlias(result, aliasType(result, "C")).kind).toBe("String");
  });

  it("model property records alias type and alias points at target", () => {
    const result = expectNoErrors(`alias Name = string; model Person { name: Name; }`);
    const propType = typeById(result, property(result, "name").propertyTypeId!);
    expect(propType.kind).toBe("Alias");
    expect(propType.name).toBe("Name");
    expect(typeById(result, propType.targetId!).kind).toBe("String");
  });

  it("allows recursive model expression aliases", () => {
    const result = expectNoErrors(`alias A = { a: A };`);
    const target = typeById(result, aliasType(result, "A").targetId!);
    expect(target.kind).toBe("Model");
    const prop = result.types.find((x) => x.kind === "ModelProperty" && x.name === "a")!;
    expect(typeById(result, prop.propertyTypeId!).id).toBe(aliasType(result, "A").id);
  });

  it("allows model spreads through aliases", () => {
    const result = expectNoErrors(`model Foo { x: string; } alias A = Foo; model Bar { ...A }`);
    const bar = namedType(result, "Model", "Bar");
    expect(bar.propertyNames).toEqual(["x"]);
  });

  it("aliases to model can be used in model is", () => {
    const result = expectNoErrors(
      `model Base { id: string; } alias BaseAlias = Base; model Derived is BaseAlias;`,
    );
    const derived = namedType(result, "Model", "Derived");
    expect(derived.propertyNames).toContain("id");
  });

  it("aliases to model can be used in model extends", () => {
    const result = expectNoErrors(
      `model Base { id: string; } alias BaseAlias = Base; model Derived extends BaseAlias { name: string; }`,
    );
    const derived = namedType(result, "Model", "Derived");
    expect(typeById(result, derived.propertyIds[0]).name).toBe("name");
  });

  it("aliases to scalar can be used in scalar extends", () => {
    const result = expectNoErrors(`alias Base = string; scalar Email extends Base;`);
    const email = namedType(result, "Scalar", "Email");
    expect(typeById(result, email.targetId ?? email.baseTypeId!).kind).toBe("String");
  });

  it("canonicalizes aliases to repeated string literals", () => {
    const result = expectNoErrors(`alias A = "same"; alias B = "same";`);
    expect(
      result.types.filter((x) => x.kind === "StringLiteral" && x.literalString === "same"),
    ).toHaveLength(1);
    expect(aliasType(result, "A").targetId).toBe(aliasType(result, "B").targetId);
  });

  it("canonicalizes aliases to repeated numeric literals", () => {
    const result = expectNoErrors(`alias A = 42; alias B = 42;`);
    expect(
      result.types.filter((x) => x.kind === "NumericLiteral" && x.literalNumeric === 42),
    ).toHaveLength(1);
    expect(aliasType(result, "A").targetId).toBe(aliasType(result, "B").targetId);
  });

  it("canonicalizes aliases to repeated boolean literals", () => {
    const result = expectNoErrors(`alias A = true; alias B = true;`);
    expect(
      result.types.filter((x) => x.kind === "BooleanLiteral" && x.literalBoolean === true),
    ).toHaveLength(1);
    expect(aliasType(result, "A").targetId).toBe(aliasType(result, "B").targetId);
  });

  it("reports self circular alias like TS", async () => {
    await expectDiagnosticsLikeTs(`alias A = A;`, ["circular-alias-type"]);
  }, 10000);

  it("reports two-node circular alias", () => {
    const result = rust(`alias A = B; alias B = A;`);
    expect(result.diagnostics.filter((x) => x.code === "circular-alias-type")).toHaveLength(1);
  });

  it("reports decorators on aliases as invalid", () => {
    const result = rust(`@doc("alias") alias A = string;`);
    expect(result.diagnostics.some((x) => x.code === "invalid-decorator-location")).toBe(true);
  });
});
