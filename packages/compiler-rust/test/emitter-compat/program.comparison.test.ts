import { describe, expect, it } from "vitest";
import type { Program as TsProgram } from "../../../compiler/src/core/program.js";
import { navigateProgram as tsNavigateProgram } from "../../../compiler/src/core/semantic-walker.js";
import type {
  SemanticNodeListener as TsSemanticNodeListener,
  Type as TsType,
} from "../../../compiler/src/core/types.js";
import {
  getDeprecated as tsGetDeprecated,
  getDoc as tsGetDoc,
  getKeyName as tsGetKeyName,
  getSummary as tsGetSummary,
} from "../../../compiler/src/lib/decorators.js";
import { Tester } from "../../../compiler/test/tester.js";
import { createIntrinsicDecoratorRegistry, check as rustCheck } from "../../index.js";
import { createProgram, type Model, type Program as RustProgram } from "../../src/program.js";
import type { SemanticNodeListener } from "../../src/walker.js";
import {
  runTestEmitter,
  rustEmitterRuntime,
  traceNavigation,
  type EmitterRuntime,
} from "./test-emitter.js";

const tsEmitterRuntime: EmitterRuntime<TsProgram> = {
  navigateProgram(program, listener) {
    tsNavigateProgram(program, listener as unknown as TsSemanticNodeListener);
  },
  getDoc: (program, target) => tsGetDoc(program, target as unknown as TsType),
  getSummary: (program, target) => tsGetSummary(program, target as unknown as TsType),
  getDeprecated: (program, target) => tsGetDeprecated(program, target as unknown as TsType),
  getKey: (program, target) => tsGetKeyName(program, target as unknown as TsType),
};

const rustTraceRuntime: Pick<EmitterRuntime<RustProgram>, "navigateProgram"> = {
  navigateProgram(program, listener: SemanticNodeListener) {
    rustEmitterRuntime.navigateProgram(program, listener);
  },
};

const tsTraceRuntime: Pick<EmitterRuntime<TsProgram>, "navigateProgram"> = {
  navigateProgram(program, listener: SemanticNodeListener) {
    tsNavigateProgram(program, listener as unknown as TsSemanticNodeListener);
  },
};

describe("emitter adapter: Rust Program vs TS Program", () => {
  const cases: Array<{ name: string; source: string }> = [
    { name: "empty file", source: "" },
    {
      name: "single model with primitive properties",
      source: "model Foo { name: string; age: int32; ok: boolean; }",
    },
    { name: "nested model references", source: "model A { b: B; } model B { x: string; }" },
    {
      name: "model and property doc",
      source: '@doc("a foo") model Foo { @doc("the id") @key id: string; }',
    },
    {
      name: "summary decorator",
      source: '@summary("short") model Foo {}',
    },
    {
      name: "doc comments",
      source: "/** Comment doc */ model Foo { /** property doc */ name: string; }",
    },
    {
      name: "enum with values and member doc",
      source: '@doc("choices") enum Choice { @doc("one") one: "one", two: 2 }',
    },
    {
      name: "union with named variants",
      source: '@doc("result") union Result { @doc("ok") ok: string, err: int32 }',
    },
    {
      name: "operation with params and return",
      source: '@doc("reads") op read(id: string, verbose?: boolean): int32;',
    },
    {
      name: "interface with multiple operations",
      source:
        '@doc("service") interface Service { @doc("get") get(id: string): string; list(): string; }',
    },
    {
      name: "nested namespaces and types",
      source: "namespace A.B { model Foo { x: string; } op read(): Foo; }",
    },
    {
      name: "templated model instantiated multiple times",
      source: "model Box<T> { value: T; } model Uses { a: Box<string>; b: Box<int32>; }",
    },
    { name: "mutual recursion", source: "model A { b: B; } model B { a: A; }" },
    {
      name: "alias to literal through property",
      source: 'alias Name = "Widget"; model Foo { name: Name; }',
    },
    { name: "augment doc on model", source: 'model Foo {} @@doc(Foo, "augmented");' },
    {
      name: "augment doc on property",
      source: 'model Foo { id: string; } @@doc(Foo.id, "identifier");',
    },
    {
      name: "model inheritance chain",
      source: "model Base { id: string; } model Child extends Base { name: string; }",
    },
    {
      name: "scalar extends primitive",
      source: "scalar Email extends string; model User { email: Email; }",
    },
    {
      name: "namespace interface and union",
      source:
        "namespace Services { union Value { s: string, n: int32 } interface Api { get(): Value; } }",
    },
  ];

  for (const { name, source } of cases) {
    it(
      name,
      async () => {
        const rustProgram = compileRust(source);
        const tsProgram = await compileTs(source);

        expect(runTestEmitter(rustProgram, rustEmitterRuntime)).toEqual(
          runTestEmitter(tsProgram, tsEmitterRuntime),
        );
      },
      20_000,
    );
  }

  it("preserves reference identity for circular models and properties", () => {
    const program = compileRust("model Foo { bar: Bar; } model Bar { foo: Foo; }");
    const global = program.getGlobalNamespaceType();
    const foo = global.models.get("Foo");
    const bar = global.models.get("Bar");
    expect(foo).toBeDefined();
    expect(bar).toBeDefined();
    const fooModel = foo as Model;
    const barModel = bar as Model;
    const fooBar = fooModel.properties.get("bar");
    const barFoo = barModel.properties.get("foo");

    expect(fooBar?.model).toBe(fooModel);
    expect(barFoo?.model).toBe(barModel);
    expect(fooBar?.type).toBe(barModel);
    expect(barFoo?.type).toBe(fooModel);
    expect(global.models.get("Foo")).toBe(fooModel);
  });

  it("reads Rust @deprecated decorator state through adapter helper", () => {
    const output = runTestEmitter(
      compileRust('@deprecated("use Bar") model Foo {}'),
      rustEmitterRuntime,
    );
    expect(output.models[0]?.deprecated).toBe("use Bar");
  });

  it("matches TS semantic-walker callback order for nested namespaces and inheritance", async () => {
    const source = `
      namespace A {
        model Base { id: string; }
        model Child extends Base { name: string; }
        namespace B {
          union U { one: string, two: int32 }
          interface I { read(): Child; }
        }
      }
    `;
    const rustProgram = compileRust(source);
    const tsProgram = await compileTs(source);

    expect(traceNavigation(rustProgram, rustTraceRuntime)).toEqual(
      traceNavigation(tsProgram, tsTraceRuntime),
    );
  }, 20_000);
});

function compileRust(source: string): RustProgram {
  const result = rustCheck(source, createIntrinsicDecoratorRegistry());
  expect(result.diagnostics.filter((diagnostic) => diagnostic.severity === "error")).toEqual([]);
  return createProgram(result);
}

async function compileTs(source: string): Promise<TsProgram> {
  const result = await Tester.compile(source);
  return result.program;
}
