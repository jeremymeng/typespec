import { describe, expect, it } from "vitest";
import type { Enum } from "../../../compiler/src/core/types.js";
import { Tester } from "../../../compiler/test/tester.js";
import { check } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  nodeId?: number;
  symbolId?: number;
  propertyIds: number[];
  propertyNames: string[];
  memberIds: number[];
  memberNames: string[];
  baseTypeId?: number;
  sourceModelId?: number;
  indexerTypeId?: number;
  propertyTypeId?: number;
  optional?: boolean;
  defaultTypeId?: number;
  decoratorNodeIds: number[];
}

interface RustCheckResult {
  types: RustType[];
  symbolToType: { symbolId: number; typeId: number }[];
  nodeToType: { nodeId: number; typeId: number }[];
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

function model(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Model", name);
}

function scalar(result: RustCheckResult, name: string): RustType {
  return namedType(result, "Scalar", name);
}

function property(result: RustCheckResult, owner: RustType, name: string): RustType {
  const index = owner.propertyNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return typeById(result, owner.propertyIds[index]);
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

describe("checker models/scalars comparison: Rust vs TS", () => {
  describe("intrinsics", () => {
    const cases = [
      ["string", "String"],
      ["bytes", "Bytes"],
      ["boolean", "Boolean"],
      ["numeric", "Numeric"],
      ["integer", "Integer"],
      ["int8", "Int8"],
      ["int16", "Int16"],
      ["int32", "Int32"],
      ["int64", "Int64"],
      ["safeint", "SafeInt"],
      ["uint8", "Uint8"],
      ["uint16", "Uint16"],
      ["uint32", "Uint32"],
      ["uint64", "Uint64"],
      ["float", "Float"],
      ["float32", "Float32"],
      ["float64", "Float64"],
      ["decimal", "Decimal"],
      ["decimal128", "Decimal128"],
      ["plainDate", "PlainDate"],
      ["plainTime", "PlainTime"],
      ["utcDateTime", "UtcDateTime"],
      ["offsetDateTime", "OffsetDateTime"],
      ["duration", "Duration"],
      ["url", "Url"],
      ["unknown", "Unknown"],
      ["null", "Null"],
    ] as const;

    it.each(cases)("resolves %s to %s", (typeName, expectedKind) => {
      const result = expectNoErrors(`model Foo { value: ${typeName}; }`);
      const value = property(result, model(result, "Foo"), "value");
      expect(typeById(result, value.propertyTypeId!).kind).toBe(expectedKind);
    });

    it("gives every intrinsic a stable symbol_to_type entry when referenced", () => {
      const result = expectNoErrors(`model Foo { name: string; age: int32; ok: boolean; }`);
      const linkedKinds = new Set(result.symbolToType.map((x) => typeById(result, x.typeId).kind));
      expect(linkedKinds.has("String")).toBe(true);
      expect(linkedKinds.has("Int32")).toBe(true);
      expect(linkedKinds.has("Boolean")).toBe(true);
    });
  });

  describe("scalars", () => {
    it("creates a scalar without extends", () => {
      const result = expectNoErrors(`scalar Currency;`);
      expect(scalar(result, "Currency").baseTypeId).toBeUndefined();
    });

    const scalarExtendsCases = [
      ["Email", "string", "String"],
      ["Small", "int32", "Int32"],
      ["Amount", "decimal", "Decimal"],
      ["Flag", "boolean", "Boolean"],
    ] as const;

    it.each(scalarExtendsCases)("creates scalar %s extends %s", (name, base, expectedKind) => {
      const result = expectNoErrors(`scalar ${name} extends ${base};`);
      expect(typeById(result, scalar(result, name).baseTypeId!).kind).toBe(expectedKind);
    });

    it("creates scalar-to-scalar extends", () => {
      const result = expectNoErrors(`scalar Base extends string; scalar Child extends Base;`);
      expect(scalar(result, "Child").baseTypeId).toBe(scalar(result, "Base").id);
    });
  });

  describe("models and properties", () => {
    it("creates a simple model property", () => {
      const result = expectNoErrors(`model Foo { x: string; }`);
      const x = property(result, model(result, "Foo"), "x");
      expect(x.kind).toBe("ModelProperty");
      expect(typeById(result, x.propertyTypeId!).kind).toBe("String");
      expect(x.baseTypeId).toBe(model(result, "Foo").id);
    });

    it("preserves property insertion order", () => {
      const result = expectNoErrors(`model Foo { a: string; b: int32; c: boolean; }`);
      expect(model(result, "Foo").propertyNames).toEqual(["a", "b", "c"]);
    });

    it("marks optional properties", () => {
      const result = expectNoErrors(`model Foo { required: string; optional?: int32; }`);
      expect(property(result, model(result, "Foo"), "required").optional).toBe(false);
      expect(property(result, model(result, "Foo"), "optional").optional).toBe(true);
    });

    const defaultCases = [
      [`"hi"`, "StringLiteral", "hi"],
      [`42`, "NumericLiteral", "42"],
      [`true`, "BooleanLiteral", "true"],
      [`false`, "BooleanLiteral", "false"],
      [`null`, "Null", ""],
    ] as const;

    it.each(defaultCases)("records property default %s", (value, expectedKind, expectedName) => {
      const result = expectNoErrors(`model Foo { x?: string = ${value}; }`);
      const defaultType = typeById(
        result,
        property(result, model(result, "Foo"), "x").defaultTypeId!,
      );
      expect(defaultType.kind).toBe(expectedKind);
      if (expectedName) expect(defaultType.name).toBe(expectedName);
    });

    it("wires model extends base_model and derived model", () => {
      const result = expectNoErrors(
        `model Bar { id: string; } model Foo extends Bar { name: string; }`,
      );
      expect(model(result, "Foo").baseTypeId).toBe(model(result, "Bar").id);
    });

    it("wires model is source_model and copies properties", () => {
      const result = expectNoErrors(`model Bar { id: string; } model Foo is Bar;`);
      const foo = model(result, "Foo");
      expect(foo.sourceModelId).toBe(model(result, "Bar").id);
      expect(foo.propertyNames).toEqual(["id"]);
      expect(typeById(result, property(result, foo, "id").propertyTypeId!).kind).toBe("String");
    });

    it("creates anonymous model expressions", () => {
      const result = expectNoErrors(`op read(): { x: string; y?: int32; };`);
      const anonymous = result.types.find(
        (x) => x.kind === "Model" && x.name === "" && x.propertyNames.includes("x"),
      );
      expect(anonymous).toBeDefined();
      expect(anonymous!.propertyNames).toEqual(["x", "y"]);
    });

    it("creates array expression models with indexers", () => {
      const result = expectNoErrors(`model Foo { values: string[]; }`);
      const values = property(result, model(result, "Foo"), "values");
      const array = typeById(result, values.propertyTypeId!);
      expect(array.kind).toBe("Model");
      expect(array.indexerTypeId).toBeDefined();
      expect(typeById(result, array.indexerTypeId!).kind).toBe("String");
    });
  });

  describe("circular references", () => {
    it("model can reference itself", () => {
      const result = expectNoErrors(`model Node { next?: Node; }`);
      expect(property(result, model(result, "Node"), "next").propertyTypeId).toBe(
        model(result, "Node").id,
      );
    });

    it("models can reference each other", () => {
      const result = expectNoErrors(`model A { b: B; } model B { a: A; }`);
      expect(property(result, model(result, "A"), "b").propertyTypeId).toBe(model(result, "B").id);
      expect(property(result, model(result, "B"), "a").propertyTypeId).toBe(model(result, "A").id);
    });

    it("model can reference itself in an array", () => {
      const result = expectNoErrors(`model Node { children: Node[]; }`);
      const array = typeById(
        result,
        property(result, model(result, "Node"), "children").propertyTypeId!,
      );
      expect(array.indexerTypeId).toBe(model(result, "Node").id);
    });

    it("models can reference each other across namespaces", () => {
      const result = expectNoErrors(`
        namespace A { model Item { other: B.Item; } }
        namespace B { model Item { other: A.Item; } }
      `);
      const models = result.types.filter((x) => x.kind === "Model" && x.name === "Item");
      expect(models).toHaveLength(2);
      const [aItem, bItem] = models;
      expect(property(result, aItem, "other").propertyTypeId).toBe(bItem.id);
      expect(property(result, bItem, "other").propertyTypeId).toBe(aItem.id);
    });
  });

  describe("decorators", () => {
    it("records model decorators without executing them", () => {
      const result = expectNoErrors(`@doc("hello") model Foo {}`);
      expect(model(result, "Foo").decoratorNodeIds).toHaveLength(1);
    });

    it("records scalar and property decorators", () => {
      const result = expectNoErrors(`@doc("scalar") scalar S; model Foo { @doc("prop") x: S; }`);
      expect(scalar(result, "S").decoratorNodeIds).toHaveLength(1);
      expect(property(result, model(result, "Foo"), "x").decoratorNodeIds).toHaveLength(1);
    });
  });

  describe("diagnostics", () => {
    it("matches TS duplicate property diagnostic", async () => {
      await expectDiagnosticsLikeTs(`model Foo { x: string; x: int32; }`, ["duplicate-property"]);
    }, 10_000);

    it("matches TS unknown type diagnostic", async () => {
      await expectDiagnosticsLikeTs(`model Foo { x: NotAType; }`, ["invalid-ref"]);
    });

    it("matches TS model extends non-model diagnostic", async () => {
      await expectDiagnosticsLikeTs(`model Foo extends string {}`, ["extend-model"]);
    });

    it("matches TS model is non-model diagnostic", async () => {
      await expectDiagnosticsLikeTs(`model Foo is string;`, ["is-model"]);
    });

    it("matches TS scalar circular extends diagnostic", async () => {
      await expectDiagnosticsLikeTs(`scalar a extends a;`, ["circular-base-type"]);
    });

    it("matches TS model circular extends diagnostic", async () => {
      await expectDiagnosticsLikeTs(`model A extends B {} model B extends A {}`, [
        "circular-base-type",
      ]);
    });
  });

  describe("declaration expressions", () => {
    const diagnosticCodes = [
      "declaration-expression-disabled",
      "templated-declaration-in-expression",
      "duplicate-property",
      "enum-member-duplicate",
      "spread-enum",
      "union-duplicate",
    ];

    it.each([
      ["model", `model Foo { x: model { a: string }; }`],
      ["named model", `model Base {} model Foo { x: model Bar extends Base { a: string }; }`],
      ["scalar", `model Foo { x: scalar extends string; }`],
      ["enum", `model Foo { x: enum { a, a }; }`],
      ["union", `model Foo { x: union { a: string, a: int32 }; }`],
      ["templated model", `alias A = model M<T> { a: T };`],
      ["templated union", `alias A = union U<T, K> { a: T };`],
      ["nested", `model Foo { x: model { y: enum { a }; }; }`],
      ["template argument", `model W<T> { v: T; } alias A = W<model { a: string }>;`],
      [
        "templated enum spread",
        `model M<T> { x: enum E { a, ...T } } enum A { x } enum B { y } model X is M<A>; model Y is M<B>;`,
      ],
      [
        "templated enum spread of non-enum and duplicate",
        `model M<T> { x: enum E { x, ...T } } enum A { x } model X is M<A>; model Y is M<string>; model Z is M<int32>;`,
      ],
      [
        "templated enum duplicate member",
        `model M<T> { x: enum E { a, a }; t: T } model X is M<string>; model Y is M<int32>;`,
      ],
      [
        "templated enum spread inside template",
        `model M<T> { x: enum E { ...T } } model N<U> { m: M<U> } enum A { x } model X is N<A>;`,
      ],
    ])(
      "reports diagnostics like TS: %s",
      async (_name, source) => {
        await expectDiagnosticsLikeTs(source, diagnosticCodes);
      },
      10000,
    );

    it("re-checks enum declaration expressions for each template instance like TS", async () => {
      const source = `model M<T> { x: enum E { a, ...T } } enum A { x } enum B { y } model X is M<A>; model Y is M<B>;`;
      const result = rust(source);
      const enumOf = (name: string) =>
        typeById(result, property(result, model(result, name), "x").propertyTypeId!);
      const [rustX, rustY] = [enumOf("X"), enumOf("Y")];

      const [ts] = await Tester.compileAndDiagnose(source);
      const tsEnumOf = (name: string) =>
        ts.program.getGlobalNamespaceType().models.get(name)!.properties.get("x")!.type as Enum;
      const [tsX, tsY] = [tsEnumOf("X"), tsEnumOf("Y")];

      expect(rustX.kind).toBe(tsX.kind);
      expect(rustX.memberNames).toEqual([...tsX.members.keys()]);
      expect(rustY.memberNames).toEqual([...tsY.members.keys()]);
      expect(rustX.id === rustY.id).toBe(tsX === tsY);
      for (const memberId of rustY.memberIds) {
        expect(typeById(result, memberId).baseTypeId).toBe(rustY.id);
      }
    });

    it.each([
      ["model", `model Foo { x: model Bar { a: string }; }`, "Model", "Bar"],
      ["scalar", `model Foo { x: scalar S extends string; }`, "Scalar", "S"],
      ["enum", `model Foo { x: enum E { a }; }`, "Enum", "E"],
      ["union", `model Foo { x: union U { a: string }; }`, "Union", "U"],
    ])("creates %s expression type not registered in namespace", (_name, source, kind, name) => {
      const result = rust(source) as RustCheckResult & {
        types: (RustType & {
          expression?: boolean;
          modelNames: string[];
          scalarNames: string[];
          enumNames: string[];
          unionNames: string[];
        })[];
      };
      const foo = model(result, "Foo");
      const propType = typeById(result, property(result, foo, "x").propertyTypeId!) as
        (RustType & { expression?: boolean }) | undefined;
      expect(propType?.kind).toBe(kind);
      expect(propType?.name).toBe(name);
      expect(propType?.expression).toBe(true);
      const namespaces = result.types.filter((x) => x.kind === "Namespace");
      for (const ns of namespaces) {
        expect([
          ...ns.modelNames,
          ...ns.scalarNames,
          ...ns.enumNames,
          ...ns.unionNames,
        ]).not.toContain(name);
      }
      expect(foo).toMatchObject({ expression: false });
    });
  });
});
