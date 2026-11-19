import { describe, expect, it } from "vitest";
import { check, checkRelation } from "../../index.js";

interface RelationResult {
  related: boolean;
  sourceTypeId?: number;
  targetTypeId?: number;
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

interface CheckResult {
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

function relation(source: string, sourceName: string, targetName: string): RelationResult {
  return checkRelation(source, sourceName, targetName) as RelationResult;
}

function expectRelated(source: string, sourceName: string, targetName: string) {
  const result = relation(source, sourceName, targetName);
  expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
  expect(result.related).toBe(true);
}

function expectUnrelated(
  source: string,
  sourceName: string,
  targetName: string,
  code = "unassignable",
) {
  const result = relation(source, sourceName, targetName);
  expect(result.related).toBe(false);
  expect(result.diagnostics.map((x) => x.code)).toContain(code);
}

function diagnosticCodes(source: string) {
  return ((check(source) as CheckResult).diagnostics ?? []).map((x) => x.code);
}

describe("checker relations comparison: Rust", () => {
  const assignableCases = [
    ["alias string to string", `alias Source = string; alias Target = string;`, "Source", "Target"],
    [
      "alias chain to string",
      `alias A = string; alias Source = A; alias Target = string;`,
      "Source",
      "Target",
    ],
    [
      "string literal to string",
      `alias Source = "hello"; alias Target = string;`,
      "Source",
      "Target",
    ],
    ["same string literal", `alias Source = "hello"; alias Target = "hello";`, "Source", "Target"],
    ["numeric literal to int32", `alias Source = 42; alias Target = int32;`, "Source", "Target"],
    ["numeric literal to int64", `alias Source = 42; alias Target = int64;`, "Source", "Target"],
    [
      "numeric literal to numeric",
      `alias Source = 42.5; alias Target = numeric;`,
      "Source",
      "Target",
    ],
    [
      "integer literal to integer",
      `alias Source = 42; alias Target = integer;`,
      "Source",
      "Target",
    ],
    [
      "decimal literal to decimal",
      `alias Source = 42.5; alias Target = decimal;`,
      "Source",
      "Target",
    ],
    [
      "boolean literal to boolean",
      `alias Source = true; alias Target = boolean;`,
      "Source",
      "Target",
    ],
    ["same boolean literal", `alias Source = false; alias Target = false;`, "Source", "Target"],
    ["int8 to int16", `alias Source = int8; alias Target = int16;`, "Source", "Target"],
    ["int8 to int32", `alias Source = int8; alias Target = int32;`, "Source", "Target"],
    ["int32 to int64", `alias Source = int32; alias Target = int64;`, "Source", "Target"],
    ["int64 to integer", `alias Source = int64; alias Target = integer;`, "Source", "Target"],
    ["integer to numeric", `alias Source = integer; alias Target = numeric;`, "Source", "Target"],
    ["uint8 to uint16", `alias Source = uint8; alias Target = uint16;`, "Source", "Target"],
    ["uint16 to uint32", `alias Source = uint16; alias Target = uint32;`, "Source", "Target"],
    ["uint32 to uint64", `alias Source = uint32; alias Target = uint64;`, "Source", "Target"],
    ["uint64 to integer", `alias Source = uint64; alias Target = integer;`, "Source", "Target"],
    ["safeint to int64", `alias Source = safeint; alias Target = int64;`, "Source", "Target"],
    ["float32 to float64", `alias Source = float32; alias Target = float64;`, "Source", "Target"],
    ["float64 to float", `alias Source = float64; alias Target = float;`, "Source", "Target"],
    ["float to numeric", `alias Source = float; alias Target = numeric;`, "Source", "Target"],
    [
      "decimal128 to decimal",
      `alias Source = decimal128; alias Target = decimal;`,
      "Source",
      "Target",
    ],
    [
      "custom string scalar literal",
      `scalar Email extends string; alias Source = "a@b"; alias Target = Email;`,
      "Source",
      "Target",
    ],
    [
      "custom numeric scalar literal",
      `scalar Small extends int8; alias Source = 12; alias Target = Small;`,
      "Source",
      "Target",
    ],
    [
      "custom scalar subtype",
      `scalar Small extends int8; alias Source = Small; alias Target = int32;`,
      "Source",
      "Target",
    ],
    [
      "source union all to string",
      `alias Source = "read" | "write"; alias Target = string;`,
      "Source",
      "Target",
    ],
    [
      "target union any string",
      `alias Source = "read"; alias Target = "read" | "write";`,
      "Source",
      "Target",
    ],
    [
      "target union any scalar",
      `alias Source = int32; alias Target = string | int32 | boolean;`,
      "Source",
      "Target",
    ],
    [
      "source union to wider union",
      `alias Source = string | int32; alias Target = string | int32 | boolean;`,
      "Source",
      "Target",
    ],
    ["never to string", `alias Source = never; alias Target = string;`, "Source", "Target"],
    ["string to unknown", `alias Source = string; alias Target = unknown;`, "Source", "Target"],
    [
      "enum member to enum",
      `enum Status { active } alias Source = Status.active; alias Target = Status;`,
      "Source",
      "Target",
    ],
    [
      "same enum to enum",
      `enum Status { active } alias Source = Status; alias Target = Status;`,
      "Source",
      "Target",
    ],
    ["model same type", `model Source { x: string; } alias Target = Source;`, "Source", "Target"],
    [
      "model structural same shape",
      `model Source { x: string; } model Target { x: string; }`,
      "Source",
      "Target",
    ],
    [
      "model structural extra source",
      `model Source { x: string; y: int32; } model Target { x: string; }`,
      "Source",
      "Target",
    ],
    [
      "model structural optional target",
      `model Source { x: string; } model Target { x: string; y?: int32; }`,
      "Source",
      "Target",
    ],
    [
      "model inherited source",
      `model Base { x: string; } model Source extends Base {} model Target { x: string; }`,
      "Source",
      "Target",
    ],
    [
      "model inherited target",
      `model Source { x: string; } model Base { x: string; } model Target extends Base {}`,
      "Source",
      "Target",
    ],
    [
      "recursive models",
      `model Source { next?: Source; } model Target { next?: Target; }`,
      "Source",
      "Target",
    ],
    [
      "reflection model target",
      `namespace TypeSpec.Reflection { model Model {} } model Source {}`,
      "Source",
      "Model",
    ],
    [
      "reflection scalar target",
      `namespace TypeSpec.Reflection { model Scalar {} } scalar Source extends string;`,
      "Source",
      "Scalar",
    ],
  ] as const;

  it.each(assignableCases)("assignable: %s", (_name, source, sourceName, targetName) => {
    expectRelated(source, sourceName, targetName);
  });

  const unassignableCases = [
    [
      "different string literal",
      `alias Source = "hello"; alias Target = "world";`,
      "Source",
      "Target",
    ],
    [
      "string literal to numeric literal",
      `alias Source = "42"; alias Target = 42;`,
      "Source",
      "Target",
    ],
    [
      "numeric literal out of int8",
      `alias Source = 9999; alias Target = int8;`,
      "Source",
      "Target",
    ],
    ["fraction to integer", `alias Source = 1.5; alias Target = integer;`, "Source", "Target"],
    ["negative to uint8", `alias Source = -1; alias Target = uint8;`, "Source", "Target"],
    ["boolean literal mismatch", `alias Source = true; alias Target = false;`, "Source", "Target"],
    ["string to boolean", `alias Source = string; alias Target = boolean;`, "Source", "Target"],
    ["int32 to int8", `alias Source = int32; alias Target = int8;`, "Source", "Target"],
    ["integer to int64", `alias Source = integer; alias Target = int64;`, "Source", "Target"],
    ["numeric to integer", `alias Source = numeric; alias Target = integer;`, "Source", "Target"],
    ["float64 to float32", `alias Source = float64; alias Target = float32;`, "Source", "Target"],
    [
      "decimal to decimal128",
      `alias Source = decimal; alias Target = decimal128;`,
      "Source",
      "Target",
    ],
    [
      "source union not all assignable",
      `alias Source = string | int32; alias Target = string;`,
      "Source",
      "Target",
    ],
    [
      "target union no variant",
      `alias Source = boolean; alias Target = string | int32;`,
      "Source",
      "Target",
    ],
    [
      "model missing required",
      `model Source {} model Target { x: string; }`,
      "Source",
      "Target",
      "missing-property",
    ],
    [
      "model optional source required target",
      `model Source { x?: string; } model Target { x: string; }`,
      "Source",
      "Target",
      "property-required",
    ],
    [
      "model property type mismatch",
      `model Source { x: int32; } model Target { x: string; }`,
      "Source",
      "Target",
      "property-unassignable",
    ],
    [
      "enum mismatch",
      `enum A { one } enum B { one } alias Source = A; alias Target = B;`,
      "Source",
      "Target",
    ],
    [
      "enum member to wrong enum",
      `enum A { one } enum B { one } alias Source = A.one; alias Target = B;`,
      "Source",
      "Target",
    ],
  ] as const;

  it.each(unassignableCases)(
    "unassignable: %s",
    (_name, source, sourceName, targetName, code = "unassignable") => {
      expectUnrelated(source, sourceName, targetName, code);
    },
  );

  // Union variants are named `recur` instead of upstream's `self`: the Rust parser rejects reserved
  // keywords as union variant names (see docs/rust-port/upstream-sync.md, Known behavioral differences).
  describe("recursive types", () => {
    const recursiveAssignableCases = [
      [
        "model referencing itself",
        `model Source { self: Source } model Target { self: Target }`,
        "Source",
        "Target",
      ],
      [
        "mutually recursive models",
        `model SourceA { b: SourceB } model SourceB { a: SourceA }
         model TargetA { b: TargetB } model TargetB { a: TargetA }`,
        "SourceA",
        "TargetA",
      ],
      [
        "union referencing itself",
        `union Loop { recur: Loop } model Target { name: string }`,
        "Loop",
        "Target",
      ],
      [
        "mutually recursive unions",
        `union LoopA { toB: LoopB } union LoopB { toA: LoopA } model Target { name: string }`,
        "LoopA",
        "Target",
      ],
      [
        "model with a recursive union property",
        `union Loop { recur: Loop, str: string } model Source { value: Loop } model Target { value: Loop }`,
        "Source",
        "Target",
      ],
      [
        "recursive union as the target",
        `union Loop { recur: Loop, str: string } alias Source = string;`,
        "Source",
        "Loop",
      ],
      [
        "mutually recursive unions on both sides",
        `union SourceA { sb: SourceB } union SourceB { sa: SourceA, ss: string }
         union TargetA { tb: TargetB } union TargetB { ta: TargetA, ts: string }`,
        "SourceA",
        "TargetA",
      ],
      [
        "model with a recursive array property",
        `model Source { items: Source[] } model Target { items: Target[] }`,
        "Source",
        "Target",
      ],
    ] as const;

    it.each(recursiveAssignableCases)("assignable: %s", (_name, source, sourceName, targetName) => {
      expectRelated(source, sourceName, targetName);
    });

    const recursiveUnassignableCases = [
      [
        "mutually recursive models with a mismatch deep in the cycle",
        `model SourceA { b: SourceB } model SourceB { a: SourceA, extra: string }
         model TargetA { b: TargetB } model TargetB { a: TargetA, extra: int32 }`,
        "SourceA",
        "TargetA",
        "property-unassignable",
      ],
      [
        "union referencing itself alongside an unassignable variant",
        `union Loop { recur: Loop, other: string } model Target { name: string }`,
        "Loop",
        "Target",
        "unassignable",
      ],
      [
        "recursive union as the target without a matching variant",
        `union Loop { recur: Loop } alias Source = string;`,
        "Source",
        "Loop",
        "unassignable",
      ],
    ] as const;

    it.each(recursiveUnassignableCases)(
      "unassignable: %s",
      (_name, source, sourceName, targetName, code) => {
        expectUnrelated(source, sourceName, targetName, code);
      },
    );

    it("checks mutually recursive models through a template constraint", () => {
      const result = check(`
        model SourceA { b: SourceB }
        model SourceB { a: SourceA }
        model TargetA { b: TargetB }
        model TargetB { a: TargetA }

        model Constrained<T extends TargetA> {}
        alias Result = Constrained<SourceA>;
      `) as CheckResult;
      // Only errors: Rust always reports the `unused-template-parameter` warning, which is a
      // linter rule in TS (see docs/rust-port/upstream-sync.md, Known behavioral differences).
      expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
    });
  });

  const defaultCases = [
    ["string literal default", `model X { a: string = "hi"; }`, false],
    ["numeric in int32 default", `model X { a: int32 = 42; }`, false],
    ["numeric out of int8 default", `model X { a: int8 = 9999; }`, true],
    ["fraction into integer default", `model X { a: integer = 1.5; }`, true],
    ["boolean default", `model X { a: boolean = true; }`, false],
    ["literal union default", `model X { a: "read" | "write" = "read"; }`, false],
    ["literal union mismatch default", `model X { a: "read" | "write" = "delete"; }`, true],
  ] as const;

  it.each(defaultCases)("checks property default assignability: %s", (_name, source, hasError) => {
    const codes = diagnosticCodes(source);
    expect(codes.includes("unassignable")).toBe(hasError);
  });
});
