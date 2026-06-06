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
  effectiveTypeId?: number;
  targetId?: number;
}

interface RustCheckResult {
  types: RustType[];
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

function rust(source: string): RustCheckResult {
  return check(source) as RustCheckResult;
}

function expectNoErrors(source: string): RustCheckResult {
  const result = rust(source);
  expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
  return result;
}

function model(result: RustCheckResult, name: string): RustType {
  const found = result.types.find((x) => x.kind === "Model" && x.name === name);
  expect(found).toBeDefined();
  return found!;
}

function typeById(result: RustCheckResult, id: number): RustType {
  return result.types[id];
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

function propertyType(result: RustCheckResult, owner: RustType, propName: string): RustType {
  const index = owner.propertyNames.indexOf(propName);
  expect(index).toBeGreaterThanOrEqual(0);
  const prop = typeById(result, owner.propertyIds[index]);
  return nonAlias(result, typeById(result, prop.propertyTypeId!));
}

function aliasTarget(result: RustCheckResult, name: string): RustType {
  const alias = result.types.find((x) => x.kind === "Alias" && x.name === name);
  expect(alias).toBeDefined();
  return typeById(result, alias!.targetId!);
}

function operationReturnType(result: RustCheckResult, name: string): RustType {
  const op = result.types.find((x) => x.kind === "Operation" && x.name === name);
  expect(op).toBeDefined();
  return typeById(result, op!.returnTypeId!);
}

describe("checker effective type comparison: Rust", () => {
  const effectiveCases = [
    ["spread", `model Source { prop: string; } model Test { prop: { ...Source }; }`, "Source"],
    [
      "indirect spread",
      `model Source { prop: string; } alias Spread = { ...Source }; model Test { prop: { ...Spread }; }`,
      "Source",
    ],
    [
      "intersect empty right",
      `model Source { prop: string; } model Test { prop: Source & {}; }`,
      "Source",
    ],
    [
      "intersect empty left",
      `model Source { prop: string; } model Test { prop: {} & Source; }`,
      "Source",
    ],
    [
      "extends spread",
      `model Base { propBase: string; } model Derived extends Base { propDerived: string; } model Test { prop: { ...Derived }; }`,
      "Derived",
    ],
    [
      "extends intersect",
      `model Base { propBase: string; } model Derived extends Base { propDerived: string; } model Test { prop: Derived & {}; }`,
      "Derived",
    ],
    [
      "spread model with two props",
      `model Source { a: string; b: string; } model Test { prop: { ...Source }; }`,
      "Source",
    ],
    [
      "spread source through alias",
      `model Source { a: string; } alias SourceAlias = Source; model Test { prop: { ...SourceAlias }; }`,
      "Source",
    ],
    [
      "nested spread source",
      `model Source { a: string; } alias A = { ...Source }; alias B = { ...A }; model Test { prop: { ...B }; }`,
      "Source",
    ],
    [
      "intersection source alias",
      `model Source { a: string; } alias A = Source & {}; model Test { prop: { ...A }; }`,
      "Source",
    ],
    [
      "intersection preserves derived",
      `model Base { a: string; } model Source extends Base { b: string; } alias A = Source & {}; model Test { prop: { ...A }; }`,
      "Source",
    ],
    ["operation return spread", `model Source { a: string; } op read(): { ...Source };`, "Source"],
    ["operation return intersect", `model Source { a: string; } op read(): Source & {};`, "Source"],
    ["model expression in alias", `model Source { a: string; } alias A = { ...Source };`, "Source"],
    ["empty source with spread", `model Source {} model Test { prop: { ...Source }; }`, "self"],
  ] as const;

  it.each(effectiveCases)("computes effective type for %s", (_name, source, expected) => {
    const result = expectNoErrors(source);
    const anonymous = source.includes("op read")
      ? operationReturnType(result, "read")
      : result.types.find(
          (x) =>
            x.kind === "Model" &&
            x.name === "" &&
            x.effectiveTypeId !== undefined &&
            x.propertyNames.length >= 0,
        );
    expect(anonymous).toBeDefined();
    const effective = typeById(result, anonymous!.effectiveTypeId!);
    if (expected === "self") {
      expect(effective.id).toBe(anonymous!.id);
    } else {
      expect(effective.id).toBe(model(result, expected).id);
    }
  });

  const propertyCases = [
    [
      "property spread",
      `model Source { a: string; } model Test { test: { ...Source }; }`,
      "test",
      "Source",
    ],
    [
      "property intersection",
      `model Source { a: string; } model Test { test: Source & {}; }`,
      "test",
      "Source",
    ],
    [
      "property indirect",
      `model Source { a: string; } alias A = { ...Source }; model Test { test: { ...A }; }`,
      "test",
      "Source",
    ],
    [
      "property derived",
      `model Base { a: string; } model Source extends Base { b: string; } model Test { test: { ...Source }; }`,
      "test",
      "Source",
    ],
    ["property empty", `model Test { test: {}; }`, "test", "self"],
    [
      "property unsourced",
      `model Source { a: string; } model Test { test: { extra: string; ...Source }; }`,
      "test",
      "self",
    ],
    [
      "property different sources",
      `model One { one: string; } model Two { two: string; } model Test { test: One & Two; }`,
      "test",
      "self",
    ],
    [
      "property nested intersection",
      `model Source { a: string; } model Test { test: (Source & {}) & {}; }`,
      "test",
      "Source",
    ],
    [
      "property nested spread",
      `model Source { a: string; } alias A = { ...Source }; model Test { test: { ...A }; }`,
      "test",
      "Source",
    ],
    [
      "property source with literals",
      `model Source { a: "x"; b: 1; } model Test { test: { ...Source }; }`,
      "test",
      "Source",
    ],
    [
      "property source with union",
      `model Source { a: "x" | "y"; } model Test { test: { ...Source }; }`,
      "test",
      "Source",
    ],
    [
      "property source with array",
      `model Source { a: string[]; } model Test { test: { ...Source }; }`,
      "test",
      "Source",
    ],
    [
      "property source alias target",
      `model Source { a: string; } alias A = Source; model Test { test: { ...A }; }`,
      "test",
      "Source",
    ],
    [
      "property source through two aliases",
      `model Source { a: string; } alias A = Source; alias B = A; model Test { test: { ...B }; }`,
      "test",
      "Source",
    ],
    [
      "property source through intersection alias",
      `model Source { a: string; } alias A = Source & {}; alias B = A & {}; model Test { test: B; }`,
      "test",
      "Source",
    ],
  ] as const;

  it.each(propertyCases)("serializes effective type on %s", (_name, source, prop, expected) => {
    const result = expectNoErrors(source);
    const propType = propertyType(result, model(result, "Test"), prop);
    const effective = typeById(result, propType.effectiveTypeId!);
    if (expected === "self") {
      expect(effective.id).toBe(propType.id);
    } else {
      expect(effective.id).toBe(model(result, expected).id);
    }
  });

  const selfCases = [
    ["named model", `model Source { a: string; }`, "Source"],
    ["anonymous empty", `alias A = {};`, "A"],
    ["anonymous unsourced property", `alias A = { a: string; };`, "A"],
    [
      "different sources",
      `model One { one: string; } model Two { two: string; } alias A = One & Two;`,
      "A",
    ],
    [
      "source plus unsourced",
      `model Source { a: string; } alias A = { b: string; } & Source;`,
      "A",
    ],
    ["partial source", `model Source { a: string; b: string; } alias A = { a: string; };`, "A"],
    ["single empty source", `model Source {} alias A = { ...Source };`, "A"],
    ["intersection empty only", `alias A = {} & {};`, "A"],
    ["expression own properties", `model Test { test: { a: string; b: string; }; }`, "property"],
    [
      "spread plus own",
      `model Source { a: string; } model Test { test: { ...Source; b: string; }; }`,
      "property",
    ],
  ] as const;

  it.each(selfCases)("keeps effective type as self for %s", (_name, source, query) => {
    const result = expectNoErrors(source);
    const target =
      query === "property"
        ? propertyType(result, model(result, "Test"), "test")
        : query === "Source"
          ? model(result, "Source")
          : aliasTarget(result, query);
    expect(target.effectiveTypeId).toBe(target.id);
  });
});
