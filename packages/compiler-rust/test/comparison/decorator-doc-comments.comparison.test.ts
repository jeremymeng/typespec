import { describe, expect, it } from "vitest";
import { getDoc } from "../../../compiler/src/lib/decorators.js";
import { Tester } from "../../../compiler/test/tester.js";
import { check, createIntrinsicDecoratorRegistry } from "../../index.js";

function stateFor(source: string, kind: string, name: string, key = "@doc:doc") {
  const result = check(source, createIntrinsicDecoratorRegistry());
  const type = result.types.find((x) => x.kind === kind && x.name === name);
  if (!type) throw new Error(`Missing ${kind} ${name}`);
  return result.decoratorState.find((x) => x.typeId === type.id && x.key === key)?.valueString;
}

describe("decorator callbacks comparison: doc comments", () => {
  it("matches TS model doc comments", async () => {
    const source = `/** hello */ model /*target*/Foo {}`;
    const ts = await Tester.compile(source);
    expect(stateFor(source, "Model", "Foo")).toBe(getDoc(ts.program, ts.target));
  }, 15_000);

  it("matches TS multiline model doc comments", async () => {
    const source = `
      /**
       * Line one.
       * Line two.
       */
      model /*target*/Foo {}
    `;
    const ts = await Tester.compile(source);
    expect(stateFor(source, "Model", "Foo")).toBe(getDoc(ts.program, ts.target));
  }, 15_000);

  it("explicit @doc overrides doc comment", () => {
    const source = `/** comment */ @doc("explicit") model Foo {}`;
    expect(stateFor(source, "Model", "Foo")).toBe("explicit");
  });

  it("matches TS property doc comments", async () => {
    const source = `model Foo { /** property doc */ /*target*/id: string; }`;
    const ts = await Tester.compile(source);
    const result = check(source, createIntrinsicDecoratorRegistry());
    const foo = result.types.find((x) => x.kind === "Model" && x.name === "Foo")!;
    const id = result.types[foo.propertyIds[foo.propertyNames.indexOf("id")]];
    expect(
      result.decoratorState.find((x) => x.typeId === id.id && x.key === "@doc:doc")?.valueString,
    ).toBe(getDoc(ts.program, ts.target));
  }, 15_000);

  it("attaches docs to enum members and union variants", () => {
    const result = check(
      `
        enum E { /** one doc */ one }
        union U { /** variant doc */ v: string }
      `,
      createIntrinsicDecoratorRegistry(),
    );
    const one = result.types.find((x) => x.kind === "EnumMember" && x.name === "one")!;
    const variant = result.types.find((x) => x.kind === "UnionVariant" && x.name === "v")!;
    expect(
      result.decoratorState.find((x) => x.typeId === one.id && x.key === "@doc:doc")?.valueString,
    ).toBe("one doc");
    expect(
      result.decoratorState.find((x) => x.typeId === variant.id && x.key === "@doc:doc")
        ?.valueString,
    ).toBe("variant doc");
  });
});
