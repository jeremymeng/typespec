import { describe, expect, it } from "vitest";
import { getDoc, getSummary } from "../../../compiler/src/lib/decorators.js";
import { Tester } from "../../../compiler/test/tester.js";
import { check, createIntrinsicDecoratorRegistry } from "../../index.js";

function checked(source: string) {
  const result = check(source, createIntrinsicDecoratorRegistry());
  expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
  return result;
}

function named(result: ReturnType<typeof checked>, kind: string, name: string) {
  const type = result.types.find((x) => x.kind === kind && x.name === name);
  if (!type) throw new Error(`Missing ${kind} ${name}`);
  return type;
}

function prop(result: ReturnType<typeof checked>, modelName: string, propName: string) {
  const model = named(result, "Model", modelName);
  const index = model.propertyNames.indexOf(propName);
  expect(index).toBeGreaterThanOrEqual(0);
  return result.types[model.propertyIds[index]];
}

function value(result: ReturnType<typeof checked>, typeId: number, key: string) {
  return result.decoratorState.find((x) => x.typeId === typeId && x.key === key)?.valueString;
}

describe("decorator callbacks comparison: augment decorators", () => {
  it("matches TS @@doc on a model", async () => {
    const source = `model /*target*/Foo {} @@doc(Foo, "augmented");`;
    const ts = await Tester.compile(source);
    const result = checked(source);
    expect(value(result, named(result, "Model", "Foo").id, "@doc:doc")).toBe(
      getDoc(ts.program, ts.target),
    );
  }, 15_000);

  it("matches TS @@summary on a model", async () => {
    const source = `model /*target*/Foo {} @@summary(Foo, "summary");`;
    const ts = await Tester.compile(source);
    const result = checked(source);
    expect(value(result, named(result, "Model", "Foo").id, "@summary:summary")).toBe(
      getSummary(ts.program, ts.target),
    );
  }, 15_000);

  it("matches TS @@doc on a property", async () => {
    const source = `model Foo { /*target*/id: string; } @@doc(Foo.id, "id doc");`;
    const ts = await Tester.compile(source);
    const result = checked(source);
    const id = prop(result, "Foo", "id");
    expect(value(result, id.id, "@doc:doc")).toBe(getDoc(ts.program, ts.target));
  }, 15_000);

  it("supports augment decorators declared before the target", () => {
    const result = checked(`@@doc(Foo, "early"); model Foo {}`);
    expect(value(result, named(result, "Model", "Foo").id, "@doc:doc")).toBe("early");
  });

  it("supports augment decorators inside a namespace", () => {
    const result = checked(`namespace A { model Foo {} @@doc(Foo, "inside"); }`);
    expect(value(result, named(result, "Model", "Foo").id, "@doc:doc")).toBe("inside");
  });

  it("augment @doc can override a doc comment", () => {
    const result = checked(`/** comment */ model Foo {} @@doc(Foo, "augment");`);
    expect(value(result, named(result, "Model", "Foo").id, "@doc:doc")).toBe("augment");
  });

  it("reports wrong target kind through JS validation", () => {
    const result = check(`model Foo {} @key model Bar {}`, createIntrinsicDecoratorRegistry());
    expect(result.diagnostics.map((x) => x.code)).toContain("decorator-wrong-target");
  });

  it("reports wrong arity through JS validation", () => {
    const result = check(`@doc model Foo {}`, createIntrinsicDecoratorRegistry());
    expect(result.diagnostics.map((x) => x.code)).toContain("invalid-argument-count");
  });

  it("skips augment decorators for unrelated targets", () => {
    const result = checked(`model Foo {} model Bar {} @@doc(Foo, "foo");`);
    expect(value(result, named(result, "Model", "Foo").id, "@doc:doc")).toBe("foo");
    expect(value(result, named(result, "Model", "Bar").id, "@doc:doc")).toBeUndefined();
  });
});
