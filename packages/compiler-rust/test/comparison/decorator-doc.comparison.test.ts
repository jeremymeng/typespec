import { describe, expect, it } from "vitest";
import { getDoc, getKeyName, getSummary } from "../../../compiler/src/lib/decorators.js";
import { Tester } from "../../../compiler/test/tester.js";
import { check, createDecoratorRegistry, createIntrinsicDecoratorRegistry } from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  propertyIds: number[];
  propertyNames: string[];
}

function rust(source: string, decorators = createIntrinsicDecoratorRegistry()) {
  return check(source, decorators) as {
    types: RustType[];
    diagnostics: { severity: string; code: string; message: string }[];
    decoratorState: {
      typeId: number;
      key: string;
      valueString?: string;
      valueBoolean?: boolean;
      valueNumber?: number;
      valueTypeId?: number;
    }[];
  };
}

function named(result: ReturnType<typeof rust>, kind: string, name: string): RustType {
  const type = result.types.find((x) => x.kind === kind && x.name === name);
  if (!type) throw new Error(`Missing ${kind} ${name}`);
  return type;
}

function property(result: ReturnType<typeof rust>, owner: RustType, name: string): RustType {
  const index = owner.propertyNames.indexOf(name);
  expect(index).toBeGreaterThanOrEqual(0);
  return result.types[owner.propertyIds[index]];
}

function state(result: ReturnType<typeof rust>, type: RustType, key: string) {
  return result.decoratorState.find((x) => x.typeId === type.id && x.key === key);
}

describe("decorator callbacks comparison: direct decorators", () => {
  it("matches TS @doc on a model", async () => {
    const source = `@doc("hello") model /*target*/Foo {}`;
    const ts = await Tester.compile(source);
    const result = rust(source);
    expect(state(result, named(result, "Model", "Foo"), "@doc:doc")?.valueString).toBe(
      getDoc(ts.program, ts.target),
    );
  }, 15_000);

  it("matches TS @summary on a model", async () => {
    const source = `@summary("short") model /*target*/Foo {}`;
    const ts = await Tester.compile(source);
    const result = rust(source);
    expect(state(result, named(result, "Model", "Foo"), "@summary:summary")?.valueString).toBe(
      getSummary(ts.program, ts.target),
    );
  }, 15_000);

  it("stores @deprecated state on a model", () => {
    const source = `@deprecated("use Bar") model Foo {}`;
    const result = rust(source);
    expect(
      state(result, named(result, "Model", "Foo"), "@deprecated:deprecated")?.valueString,
    ).toBe("use Bar");
  });

  it("matches TS property @doc", async () => {
    const source = `model Foo { @doc("the id") /*target*/id: string; }`;
    const ts = await Tester.compile(source);
    const result = rust(source);
    const id = property(result, named(result, "Model", "Foo"), "id");
    expect(state(result, id, "@doc:doc")?.valueString).toBe(getDoc(ts.program, ts.target));
  }, 15_000);

  it("matches TS @key with implicit property name", async () => {
    const source = `model Foo { @key /*target*/id: string; }`;
    const ts = await Tester.compile(source);
    const result = rust(source);
    const id = property(result, named(result, "Model", "Foo"), "id");
    expect(state(result, id, "@key:key")?.valueString).toBe(getKeyName(ts.program, ts.target));
  }, 15_000);

  it("matches TS @key with alternate name", async () => {
    const source = `model Foo { @key("wireId") /*target*/id: string; }`;
    const ts = await Tester.compile(source);
    const result = rust(source);
    const id = property(result, named(result, "Model", "Foo"), "id");
    expect(state(result, id, "@key:key")?.valueString).toBe(getKeyName(ts.program, ts.target));
  }, 15_000);

  it("applies chained decorators", () => {
    const result = rust(`@doc("long") @summary("short") model Foo {}`);
    const foo = named(result, "Model", "Foo");
    expect(state(result, foo, "@doc:doc")?.valueString).toBe("long");
    expect(state(result, foo, "@summary:summary")?.valueString).toBe("short");
  });

  it("stores state for scalar, enum, union, interface, and operation targets", () => {
    const result = rust(`
      @doc("scalar") scalar S;
      @doc("enum") enum E { @doc("member") one }
      @doc("union") union U { @doc("variant") v: string }
      @doc("iface") interface I { @doc("op") read(): string; }
    `);
    for (const [kind, name, expected] of [
      ["Scalar", "S", "scalar"],
      ["Enum", "E", "enum"],
      ["EnumMember", "one", "member"],
      ["Union", "U", "union"],
      ["UnionVariant", "v", "variant"],
      ["Interface", "I", "iface"],
      ["Operation", "read", "op"],
    ] as const) {
      expect(state(result, named(result, kind, name), "@doc:doc")?.valueString).toBe(expected);
    }
  });

  it("reports unknown decorator only when a registry is supplied", () => {
    expect(rust(`@notReal model Foo {}`).diagnostics.map((x) => x.code)).toContain(
      "unknown-decorator",
    );
    expect(
      check(`@notReal model Foo {}`).diagnostics.some((x) => x.code === "unknown-decorator"),
    ).toBe(false);
  });

  it("converts JS exceptions to diagnostics", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": () => {
        throw new Error("boom");
      },
    });
    const result = rust(`@doc("hello") model Foo {}`, decorators);
    expect(result.diagnostics.map((x) => x.code)).toContain("decorator-exception");
    expect(result.diagnostics[0].message).toContain("boom");
  });

  it("allows a decorator to mutate another target passed as an argument", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context, target, other, value) => {
        context.setProperty(other ?? target, "doc", value);
      },
    });
    const result = rust(`model Other {} @doc(Other, "cross") model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Other"), "@doc:doc")?.valueString).toBe("cross");
  });

  it("decorator context getProperty can read prior state", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@summary": (context, target, value) => {
        context.setProperty(target, "doc", value);
        context.setProperty(target, "summary", context.getProperty(target, "doc"));
      },
    });
    const result = rust(`@summary("from doc") model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Foo"), "@summary:summary")?.valueString).toBe(
      "from doc",
    );
  });

  it("decorator context reportDiagnostic adds a checker diagnostic", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context) =>
        context.reportDiagnostic({ code: "custom-doc", message: "custom diagnostic" }),
    });
    const result = rust(`@doc("hello") model Foo {}`, decorators);
    expect(result.diagnostics.map((x) => x.code)).toContain("custom-doc");
  });

  it("stores boolean decorator state from JS", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context, target) => context.setProperty(target, "flag", true),
    });
    const result = rust(`@doc("hello") model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Foo"), "@doc:flag")?.valueBoolean).toBe(true);
  });

  it("stores number decorator state from JS", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context, target) => context.setProperty(target, "count", 42),
    });
    const result = rust(`@doc("hello") model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Foo"), "@doc:count")?.valueNumber).toBe(42);
  });

  it("stores type reference decorator state from JS", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context, target, other) => context.setProperty(target, "other", other),
    });
    const result = rust(`model Other {} @doc(Other) model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Foo"), "@doc:other")?.valueTypeId).toBe(
      named(result, "Model", "Other").id,
    );
  });

  it("supports null decorator state from JS", () => {
    const decorators = createDecoratorRegistry({
      "TypeSpec.@doc": (context, target) => context.setProperty(target, "empty", null),
    });
    const result = rust(`@doc("hello") model Foo {}`, decorators);
    expect(state(result, named(result, "Model", "Foo"), "@doc:empty")).toBeDefined();
  });
});
