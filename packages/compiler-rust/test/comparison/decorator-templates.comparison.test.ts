import { describe, expect, it } from "vitest";
import type { DecoratorContext, Type } from "../../../compiler/src/core/types.js";
import { mockFile } from "../../../compiler/src/testing/index.js";
import { Tester } from "../../../compiler/test/tester.js";
import {
  check,
  createDecoratorRegistry,
  type DecoratorContext as RustContext,
} from "../../index.js";

interface RustType {
  id: number;
  kind: string;
  name: string;
  propertyIds: number[];
  propertyNames: string[];
  propertyTypeId?: number;
}

interface RustResult {
  types: RustType[];
  diagnostics: { severity: string; code: string; message: string }[];
}

const decl = `extern dec track(target: unknown, arg?: unknown);`;

function describeTs(value: unknown): string {
  if (value === undefined) return "-";
  if (typeof value !== "object" || value === null) return `${typeof value}:${String(value)}`;
  const type = value as Type & { name?: string | symbol };
  return `${type.kind}:${typeof type.name === "string" ? type.name : ""}`;
}

async function tsTrackCalls(source: string): Promise<string[]> {
  const calls: string[] = [];
  await Tester.files({
    "track.js": mockFile.js({
      $track: (_ctx: DecoratorContext, target: Type, arg?: unknown) => {
        calls.push(`${describeTs(target)} <- ${describeTs(arg)}`);
      },
    }),
  })
    .import("./track.js")
    .compile(`${decl}\n${source}`);
  return calls.sort();
}

function rustTrackCalls(source: string): { calls: string[]; result: RustResult } {
  const calls: string[] = [];
  const describeRust = (ctx: RustContext, value: unknown) => {
    if (value === undefined) return "-";
    if (typeof value !== "object" || value === null) return `${typeof value}:${String(value)}`;
    // getName returns the display name (`Foo<A>`); compare the bare name like TS `type.name`.
    return `${ctx.getKind(value)}:${(ctx.getName(value) ?? "").replace(/<.*$/, "")}`;
  };
  const result = check(
    `${decl}\n${source}`,
    createDecoratorRegistry({
      "@track": (ctx, target, arg) => {
        calls.push(`${describeRust(ctx, target)} <- ${describeRust(ctx, arg)}`);
      },
    }),
  ) as RustResult;
  expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
  return { calls: calls.sort(), result };
}

async function expectTrackCallsLikeTs(source: string, expected?: string[]) {
  const ts = await tsTrackCalls(source);
  if (expected) {
    expect(ts).toEqual([...expected].sort());
  }
  expect(rustTrackCalls(source).calls).toEqual(ts);
}

describe("decorators in templates comparison: Rust vs TS", () => {
  describe("does not run decorators when checking template declarations", () => {
    it.each([
      ["model", `@track(T) model Foo<T> {}`],
      ["interface", `@track(T) interface Foo<T> {}`],
      ["operation", `@track(T) op foo<T>(): void;`],
      ["union", `@track(T) union Foo<T> {}`],
      ["scalar", `@track(T) scalar Foo<T> extends string;`],
      ["model property", `model Foo<T> { @track(T) prop: string }`],
      ["alias model property", `alias Foo<T> = { @track(T) prop: string; };`],
      ["alias without argument", `alias Foo<T> = { @track prop: string; };`],
      [
        "indirect alias instantiation",
        `alias Bar<T> = Foo<T>; alias Foo<T> = { @track(T) prop: string; };`,
      ],
      [
        "nested indirect alias instantiation",
        `alias Bar<T> = { nested: Foo<T> }; alias Foo<T> = { @track(T) prop: string; };`,
      ],
      ["instance inside template", `@track(T) model Foo<T> {} model Bar<U> { foo: Foo<U> }`],
    ])("%s", async (_name, source) => {
      await expectTrackCallsLikeTs(source, []);
    });
  });

  it.each([
    ["model instance", `@track(T) model Foo<T> {} model A {} model X { foo: Foo<A> }`],
    ["union instance", `@track(T) union Foo<T> { T } model A {} model X { foo: Foo<A> }`],
    [
      "interface instance",
      `@track(T) interface Foo<T> { @track(T) op get(): T; } model A {} interface X extends Foo<A> {}`,
    ],
    ["operation instance", `@track(T) op foo<T>(): T; model A {} op bar is foo<A>;`],
    [
      "model property instance",
      `model Foo<T> { @track(T) prop: string } model A {} model X { foo: Foo<A> }`,
    ],
    ["model is instance", `model Foo<T> { @track(T) prop: string } model A {} model X is Foo<A>;`],
    [
      "model spread instance",
      `model Foo<T> { @track(T) prop: string } model A {} model X { ...Foo<A> }`,
    ],
    [
      "alias instance",
      `alias Foo<T> = { @track(T) prop: string; }; model A {} model X { foo: Foo<A> }`,
    ],
    [
      "nested instance",
      `model Foo<T> { @track(T) prop: string } model Bar<U> { foo: Foo<U> } model A {} model X { bar: Bar<A> }`,
    ],
    [
      "spread in template instance",
      `model Foo<T> { @track(T) prop: string } model Bar<U> { ...Foo<U> } model A {} model X { bar: Bar<A>; foo: Foo<A> }`,
    ],
  ])("runs decorators on template instances with resolved arguments: %s", async (_name, source) => {
    await expectTrackCallsLikeTs(source);
  });

  it("does not run decorators with unresolved template parameters from operation template parameter defaults", async () => {
    // Port of upstream 16caf10f0.
    await expectTrackCallsLikeTs(
      `
      @track(Resource)
      model TagsUpdateModel<Resource extends {}> {
        tags?: string;
      }

      op armTagsPatch<
        Resource extends {},
        Properties extends {} = TagsUpdateModel<Resource>
      >(body: Properties): void;

      model FooResource { id: string; }

      op patch is armTagsPatch<FooResource>;
      `,
      ["Model:TagsUpdateModel <- Model:FooResource"],
    );
  });
});
