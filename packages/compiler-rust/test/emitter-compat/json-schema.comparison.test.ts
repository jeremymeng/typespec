import { getTypeName, resolvePath } from "@typespec/compiler";
import { createTester, resolveVirtualPath } from "@typespec/compiler/testing";
import { describe, expect, it } from "vitest";
import { compile } from "../../src/decorator-registration.js";
import { createMemoryEmitterHost } from "../../src/host-fs.js";
import { createVirtualProgramHost } from "../../src/host.js";

// M12: `@typespec/json-schema` (through `@typespec/asset-emitter`) runs on the Program adapter
// over the Rust-checked program and must produce the same files as on the TS program.

const packageRoot = resolvePath(import.meta.dirname, "../..");
const emitterName = "@typespec/json-schema";
const Tester = createTester(packageRoot, { libraries: [emitterName] }).emit(emitterName);
const mainPath = resolveVirtualPath("main.tsp");
const header = `import "@typespec/json-schema"; using JsonSchema;`;

type EmitterOptions = Record<string, unknown>;

interface EmitResult {
  outputs: Record<string, string>;
  diagnostics: string[];
}

function normalizeDiagnostics(diagnostics: readonly { code: string; message: string }[]) {
  return diagnostics.map((d) => `${d.code}: ${d.message}`).sort();
}

async function emitTs(code: string, options: EmitterOptions): Promise<EmitResult> {
  const [{ outputs }, diagnostics] = await Tester.compileAndDiagnose(code, {
    compilerOptions: { options: { [emitterName]: options as any } },
  });
  return { outputs: { ...outputs }, diagnostics: normalizeDiagnostics(diagnostics) };
}

async function emitRust(code: string, options: EmitterOptions): Promise<EmitResult> {
  const outputHost = createMemoryEmitterHost();
  const outputDir = "/out";
  const program = await compile(mainPath, createVirtualProgramHost({ [mainPath]: code }), {
    libraries: [emitterName],
    outputHost,
    compilerOptions: {
      emit: [emitterName],
      outputDir,
      options: { [emitterName]: options },
    },
  });
  const prefix = `${outputDir}/${emitterName}/`;
  const outputs: Record<string, string> = {};
  for (const [path, content] of outputHost.files) {
    if (path.startsWith(prefix)) outputs[path.slice(prefix.length)] = content;
  }
  return {
    outputs,
    diagnostics: normalizeDiagnostics(
      program.diagnostics.filter((d) => !linterGatedCodes.has(d.code)),
    ),
  };
}

// Reported unconditionally by Rust but only by the TS linter (known difference, owner M13).
const linterGatedCodes = new Set([
  "@typespec/compiler/unused-using",
  "@typespec/compiler/unused-template-parameter",
]);

async function expectSameOutput(
  code: string,
  options: EmitterOptions = {},
  { namespace = true }: { namespace?: boolean } = {},
) {
  const source = namespace ? `${header} @jsonSchema namespace test; ${code}` : `${header} ${code}`;
  const emitterOptions = { "file-type": "json", ...options };
  const [ts, rust] = await Promise.all([
    emitTs(source, emitterOptions),
    emitRust(source, emitterOptions),
  ]);
  expect(rust.diagnostics).toEqual(ts.diagnostics);
  expect(Object.keys(rust.outputs).sort()).toEqual(Object.keys(ts.outputs).sort());
  for (const [file, content] of Object.entries(ts.outputs)) {
    expect({ file, content: rust.outputs[file] }).toEqual({ file, content });
  }
  return ts;
}

describe("json-schema emitter on the Rust adapter", () => {
  describe("models", () => {
    it("emits a model with primitive properties", async () => {
      const ts = await expectSameOutput(`model Foo { a: string; b?: int32; c: boolean; }`);
      expect(Object.keys(ts.outputs)).toEqual(["Foo.json"]);
    });

    it("emits every built-in scalar", async () => {
      await expectSameOutput(`
        model AllScalars {
          s: string; by: bytes; i8: int8; i16: int16; i32: int32; i64: int64;
          u8: uint8; u16: uint16; u32: uint32; u64: uint64; si: safeint; int: integer;
          f: float; f32: float32; f64: float64; d: decimal; d128: decimal128; n: numeric;
          b: boolean; u: url; pd: plainDate; pt: plainTime; utc: utcDateTime;
          odt: offsetDateTime; dur: duration; unk: unknown; nul: null;
        }
      `);
    });

    it("emits references between models", async () => {
      await expectSameOutput(`model Foo { bar: Bar; } model Bar { x: string; }`);
    });

    it("emits a recursive model", async () => {
      await expectSameOutput(`model Node { value: string; children: Node[]; parent?: Node; }`);
    });

    it("emits base models and model is", async () => {
      await expectSameOutput(`
        model Base { id: string; }
        model Derived extends Base { name: string; }
        model Copy is Base { extra: int32; }
      `);
    });

    it("emits model expressions and spreads", async () => {
      await expectSameOutput(`
        model Common { a: string; }
        model Foo { ...Common; inline: { x: int32; y?: string }; }
      `);
    });

    it("emits template instances", async () => {
      await expectSameOutput(`
        model Page<T> { items: T[]; next?: string; }
        model Widget { name: string; }
        model Holder { widgets: Page<Widget>; names: Page<string>; }
      `);
    });

    it("emits arrays and records", async () => {
      await expectSameOutput(`
        model Foo {
          @minItems(1) @maxItems(4) @uniqueItems tags: string[];
          counts: Record<int32>;
          nested: string[][];
        }
        model Names is string[];
        model Bag is Record<boolean>;
      `);
    });

    it("emits literal and union property types", async () => {
      await expectSameOutput(`
        model Foo {
          s: "fixed"; n: 42; b: true;
          either: string | int32;
          nullable: string | null;
          kind: "a" | "b" | "c";
        }
      `);
    });

    it("emits intersections", async () => {
      await expectSameOutput(`
        model A { a: string; } model B { b: int32; }
        model Foo { both: A & B; }
      `);
    });

    it("emits property defaults", async () => {
      await expectSameOutput(`
        enum Color { red, blue }
        model Foo {
          s?: string = "hi"; n?: int32 = 3; f?: float64 = 1.5; b?: boolean = false;
          c?: Color = Color.blue; l?: "x" | "y" = "y";
        }
      `);
    });

    it("emits array and object defaults", async () => {
      await expectSameOutput(`
        model Point { x: int32; y: int32; }
        model Foo { tags?: string[] = #["a", "b"]; origin?: Point = #{ x: 0, y: 0 }; }
      `);
    });

    it("emits a templated declaration only through instances", async () => {
      await expectSameOutput(`model Box<T> { value: T; } model UsesBox { b: Box<int64>; }`);
    });
  });

  describe("enums and unions", () => {
    it("emits enums with string and numeric members", async () => {
      await expectSameOutput(`
        enum Color { red, green: "GREEN" }
        enum Level { low: 1, high: 10 }
        model Foo { color: Color; level: Level; red: Color.red; }
      `);
    });

    it("emits named unions", async () => {
      await expectSameOutput(`
        model Cat { meow: boolean; } model Dog { bark: boolean; }
        union Pet { cat: Cat, dog: Dog, "none" }
        @oneOf union Strict { s: string, n: int32 }
      `);
    });

    it("emits discriminated models", async () => {
      await expectSameOutput(`
        @discriminator("kind") model Pet { kind: string; name: string; }
        model Cat extends Pet { kind: "cat"; meows: boolean; }
        model Dog extends Pet { kind: "dog"; barks: boolean; }
      `);
    });
  });

  describe("scalars and constraints", () => {
    it("emits custom scalars with constraints", async () => {
      await expectSameOutput(`
        @format("uuid") @pattern("^[a-f0-9-]+$") @minLength(36) @maxLength(36) scalar uuid extends string;
        @minValue(0) @maxValue(100) scalar percent extends int32;
        @minValueExclusive(0) @maxValueExclusive(1) scalar ratio extends float64;
        model Foo { id: uuid; p: percent; r: ratio; @minValue(5) count: int32; }
      `);
    });

    it("emits json-schema specific constraints", async () => {
      await expectSameOutput(`
        @minProperties(1) @maxProperties(3)
        model Foo {
          @multipleOf(5) n: int32;
          @contains(string) @minContains(1) @maxContains(2) list: unknown[];
          @contentEncoding("base64") @contentMediaType("image/png") data: string;
        }
      `);
    });

    it("emits encoded scalars", async () => {
      await expectSameOutput(`
        model Foo {
          @encode("rfc7231") created: utcDateTime;
          @encode("seconds", int32) timeout: duration;
          @encode("base64url") blob: bytes;
        }
      `);
    });
  });

  describe("documentation and metadata", () => {
    it("emits docs and summaries", async () => {
      await expectSameOutput(`
        /** A documented model. */
        @summary("Foo summary")
        model Foo {
          @doc("the a property") a: string;
          /** the b property */ b: int32;
        }
        @doc("Colors") enum Color { @doc("Red!") red }
      `);
    });

    it("emits extensions, ids and base URIs", async () => {
      await expectSameOutput(
        `
        @jsonSchema("http://example.com/schemas")
        namespace test;
        @id("custom-id") @extension("x-custom", "value") model Foo { @extension("x-prop", #{ a: 1 }) a: string; }
        model Bar { foo: Foo; }
      `,
        {},
        { namespace: false },
      );
    });

    it("emits deprecated types", async () => {
      await expectSameOutput(`
        #deprecated "Use Bar instead"
        model Foo { a: string; }
        model Bar { #deprecated "old" old?: string; }
      `);
    });

    it("emits examples", async () => {
      await expectSameOutput(`
        @example(#{ a: "x", b: 1 })
        model Foo { a: string; b: int32; }
      `);
    });
  });

  describe("namespaces and discovery", () => {
    it("emits nested namespaces", async () => {
      await expectSameOutput(
        `
        @jsonSchema namespace Outer {
          model A { b: Inner.B; }
          namespace Inner { model B { x: string; } }
        }
      `,
        {},
        { namespace: false },
      );
    });

    it("emits types marked individually", async () => {
      await expectSameOutput(
        `@jsonSchema model Foo { bar: Bar; } model Bar { x: string; } model Ignored {}`,
        {},
        { namespace: false },
      );
    });

    it("ignores interfaces and operations", async () => {
      await expectSameOutput(`model Foo {} interface I { op get(): Foo; } op standalone(): void;`);
    });
  });

  describe("emitter options", () => {
    it("emits yaml", async () => {
      await expectSameOutput(`model Foo { a: string; b: Bar; } model Bar { c: int32[]; }`, {
        "file-type": "yaml",
      });
    });

    it("emits all models", async () => {
      await expectSameOutput(
        `model Foo { a: string; } namespace Other { model Bar {} }`,
        {
          emitAllModels: true,
        },
        { namespace: false },
      );
    });

    it("emits a bundle", async () => {
      await expectSameOutput(`model Foo { a: Bar; } model Bar { b: string; }`, {
        bundleId: "bundle.json",
      });
    });

    it("emits with a custom int64 strategy", async () => {
      await expectSameOutput(`model Foo { a: int64; b: uint64; }`, { "int64-strategy": "number" });
    });
  });

  describe("tuples and string templates", () => {
    it("emits tuples", async () => {
      await expectSameOutput(`model Foo { pair: [string, int32]; }`);
    });

    it("emits string templates", async () => {
      await expectSameOutput(`
        alias Prefix = "pre";
        model Foo { a: "\${Prefix}-fixed"; b: "\${Prefix}-\${string}"; }
      `);
    });
  });

  describe("Program adapter surface", () => {
    const source = `${header}
      namespace A.B { model M<T> { t: T } model X { m: M<string>; u: string | int32; e: E } enum E { a } }
      op o(x: string): void;`;

    async function programs() {
      const { program: ts } = await createTester(packageRoot, { libraries: [emitterName] }).compile(
        source,
      );
      const rust = await compile(mainPath, createVirtualProgramHost({ [mainPath]: source }), {
        libraries: [emitterName],
      });
      return { ts, rust };
    }

    function namedTypes(program: any) {
      const b = program.getGlobalNamespaceType().namespaces.get("A").namespaces.get("B");
      const x = b.models.get("X");
      return [
        b,
        x,
        ...[...x.properties.values()].flatMap((p: any) => [p, p.type]),
        b.enums.get("E").members.get("a"),
        program.getGlobalNamespaceType().operations.get("o"),
      ];
    }

    it("names types like the TS getTypeName", async () => {
      const { ts, rust } = await programs();
      const names = (program: any) => namedTypes(program).map((type) => getTypeName(type));
      expect(names(rust)).toEqual(names(ts));
      expect(namedTypes(rust).map((type) => rust.checker.getTypeName(type))).toEqual(names(ts));
    });

    it("lists source files by path", async () => {
      const { ts, rust } = await programs();
      // Library paths differ between the TS test file system and the Rust host.
      const files = (program: any) =>
        [...program.sourceFiles.keys()]
          .map((path: string) => path.replace(/^.*\/(node_modules\/@typespec|packages)\//, ""))
          .filter((path: string) => !path.startsWith("compiler/test-lib/"))
          .sort();
      expect(files(rust)).toEqual(files(ts));
      expect(rust.sourceFiles.get(mainPath)?.file.path).toBe(mainPath);
    });

    it("clones types with independent members", async () => {
      const { rust } = await programs();
      const x: any = namedTypes(rust)[1];
      const clone: any = rust.checker.cloneType(x, { name: "Y" });
      expect(clone).not.toBe(x);
      expect(clone.name).toBe("Y");
      expect(clone.kind).toBe("Model");
      expect(clone.properties).not.toBe(x.properties);
      expect([...clone.properties.keys()]).toEqual([...x.properties.keys()]);
      expect(clone.isFinished).toBe(true);
      const created: any = rust.checker.createAndFinishType({
        kind: "Model",
        name: "",
        properties: new Map(),
        decorators: [],
      });
      expect(created.isFinished).toBe(true);
      expect(created.entityKind).toBe("Type");
    });

    it("looks up standard types", async () => {
      const { ts, rust } = await programs();
      for (const name of ["string", "int32", "Array", "Record"] as const) {
        expect((rust.checker.getStdType(name) as any).name).toBe(ts.checker.getStdType(name).name);
      }
    });
  });
});
