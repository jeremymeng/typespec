import { describe, expect, it } from "vitest";
import { Numeric } from "../../../compiler/src/core/numeric.js";
import { check, checkRelation, numericEquals, numericFromSerialized } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  propertyIds: number[];
  propertyNames: string[];
  memberIds: number[];
  memberNames: string[];
  propertyTypeId?: number;
  targetId?: number;
  baseTypeId?: number;
  literalNumeric?: number;
  literalNumericBigint?: bigint;
  literalNumericString?: string;
  literalNumericNumber?: number;
  enumMemberValueNumeric?: number;
  enumMemberValueNumericBigint?: bigint;
  enumMemberValueNumericString?: string;
  enumMemberValueNumericNumber?: number;
}

interface RustCheckResult {
  types: RustType[];
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

interface RelationResult {
  related: boolean;
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
  expect(found, `Missing ${kind} ${name}`).toBeDefined();
  return found!;
}

function aliasTarget(result: RustCheckResult, name: string): RustType {
  return typeById(result, namedType(result, "Alias", name).targetId!);
}

function propertyTarget(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.propertyNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, typeById(result, owner.propertyIds[index]).propertyTypeId!);
}

function relation(source: string, sourceName = "Source", targetName = "Target"): RelationResult {
  return checkRelation(source, sourceName, targetName) as RelationResult;
}

describe("checker numeric precision comparison: Rust vs TS", () => {
  it.each([
    ["int64 max", "9223372036854775807"],
    ["int64 max + 1", "9223372036854775808"],
    ["int64 min", "-9223372036854775808"],
    ["uint64 max", "18446744073709551615"],
    ["uint64 max + 1", "18446744073709551616"],
    ["safeint max", "9007199254740991"],
    ["safeint max + 1", "9007199254740992"],
    ["precise decimal", "1.234567890123456789012345"],
    ["float32 max range literal", "3.4e38", "340000000000000000000000000000000000000"],
  ])(
    "preserves %s literal exactly",
    (_name, literal, expected = literal) => {
      const result = expectNoErrors(`alias A = ${literal};`);
      const value = aliasTarget(result, "A");
      expect(value.kind).toBe("NumericLiteral");
      expect(value.name).toBe(Numeric(expected).toString());
      expect(numericEquals(numericFromSerialized(value), Numeric(expected))).toBe(true);
    },
    15000,
  );

  it("keeps distinct large literals as distinct TypeIds", () => {
    const result = expectNoErrors(`alias A = 9007199254740992; alias B = 9007199254740993;`);
    const a = aliasTarget(result, "A");
    const b = aliasTarget(result, "B");
    expect(a.id).not.toBe(b.id);
    expect(numericEquals(numericFromSerialized(a), Numeric("9007199254740992"))).toBe(true);
    expect(numericEquals(numericFromSerialized(b), Numeric("9007199254740993"))).toBe(true);
  });

  it("canonicalizes numerically-equal literal spellings", () => {
    const result = expectNoErrors(`alias A = 1.0; alias B = 1; alias C = 1e0;`);
    expect(aliasTarget(result, "A").id).toBe(aliasTarget(result, "B").id);
    expect(aliasTarget(result, "B").id).toBe(aliasTarget(result, "C").id);
  });

  it("does not collapse distinct precise decimals", () => {
    const result = expectNoErrors(
      `alias A = 1.234567890123456789012345; alias B = 1.234567890123456789012346; alias C = 0.3; alias D = 0.30000000000000004;`,
    );
    expect(aliasTarget(result, "A").id).not.toBe(aliasTarget(result, "B").id);
    expect(aliasTarget(result, "C").id).not.toBe(aliasTarget(result, "D").id);
  });

  it("uses exact Numeric values for scalar range checks", () => {
    const cases = [
      ["9223372036854775807", "int64", true],
      ["9223372036854775808", "int64", false],
      ["-9223372036854775808", "int64", true],
      ["18446744073709551615", "uint64", true],
      ["18446744073709551616", "uint64", false],
      ["9007199254740991", "safeint", true],
      ["9007199254740992", "safeint", false],
      ["3.4e38", "float32", true],
      ["3.4e40", "float32", false],
    ] as const;

    for (const [literal, scalar, related] of cases) {
      const result = relation(`alias Source = ${literal}; alias Target = ${scalar};`);
      expect(result.diagnostics.map((x) => x.code)).toEqual(related ? [] : ["unassignable"]);
      expect(result.related, `${literal} assignable to ${scalar}`).toBe(related);
    }
  });

  it("keeps template instantiations distinct for literals beyond Number.MAX_SAFE_INTEGER", () => {
    const result = expectNoErrors(
      `model Foo<T> { value: T; } model A { x: Foo<9007199254740992>; y: Foo<9007199254740993>; }`,
    );
    const model = namedType(result, "Model", "A");
    const x = propertyTarget(result, model, "x");
    const y = propertyTarget(result, model, "y");
    expect(x.id).not.toBe(y.id);
    expect(
      numericEquals(
        numericFromSerialized(propertyTarget(result, x, "value")),
        Numeric("9007199254740992"),
      ),
    ).toBe(true);
    expect(
      numericEquals(
        numericFromSerialized(propertyTarget(result, y, "value")),
        Numeric("9007199254740993"),
      ),
    ).toBe(true);
  });

  it("marshals enum member numeric values losslessly", () => {
    const result = expectNoErrors(`enum Limit { MaxInt64: 9223372036854775807 }`);
    const limit = namedType(result, "Enum", "Limit");
    const member = typeById(result, limit.memberIds[0]);
    expect(member.enumMemberValueNumericBigint).toBe(9223372036854775807n);
    expect(member.enumMemberValueNumeric).toBeUndefined();
    expect(member.enumMemberValueNumericNumber).toBeUndefined();
  });
});
