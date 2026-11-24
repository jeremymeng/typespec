import { describe, expect, it } from "vitest";
import { getSourceLocation } from "../../../compiler/src/core/diagnostics.js";
import { resolveVirtualPath } from "../../../compiler/src/testing/fs.js";
import { Tester } from "../../../compiler/test/tester.js";
import type { CheckResultJs } from "../../index.js";
import { checkProgram, createVirtualProgramHost, getCompilerRoot } from "../../src/host.js";
import { createProgram } from "../../src/program.js";

// M10: the Rust checker loads the real standard library (`@typespec/compiler`
// `lib/intrinsics.tsp` + `lib/std/main.tsp`) like the TS compiler does.

interface NormalizedDiagnostic {
  code: string;
  message: string;
  pos?: number;
  end?: number;
}

const mainPath = resolveVirtualPath("main.tsp");

function sortDiagnostics(diagnostics: NormalizedDiagnostic[]) {
  return [...diagnostics].sort(
    (a, b) => (a.pos ?? -1) - (b.pos ?? -1) || a.code.localeCompare(b.code),
  );
}

async function compileTs(source: string) {
  const [{ program }, diagnostics] = await Tester.compileAndDiagnose(source, {
    // Linter rules in TS; always reported for project files by the Rust checker.
    compilerOptions: {
      linterRuleSet: {
        enable: {
          "@typespec/compiler/unused-using": true,
          "@typespec/compiler/unused-template-parameter": true,
        },
      },
    },
  });
  return {
    program,
    diagnostics: sortDiagnostics(
      diagnostics.map((d) => {
        const location = getSourceLocation(d.target);
        expect(location?.file.path ?? mainPath).toBe(mainPath);
        return {
          code: d.code.replace(/^@typespec\/compiler\//, ""),
          message: d.message,
          ...(location ? { pos: location.pos, end: location.end } : {}),
        };
      }),
    ),
  };
}

function compileRust(source: string) {
  const result = checkProgram(mainPath, createVirtualProgramHost({ [mainPath]: source }));
  const diagnostics = result.diagnostics.map((d) => {
    expect(d.file ?? mainPath).toBe(mainPath);
    return {
      code: d.code,
      message: d.message,
      ...(d.file !== undefined ? { pos: d.pos, end: d.end } : {}),
    };
  });
  return {
    result,
    program: createProgram(result as unknown as CheckResultJs),
    diagnostics: sortDiagnostics(diagnostics),
  };
}

/** Structural description shared by the TS and Rust (adapter) type graphs. */
function describeType(type: any, depth = 0): unknown {
  if (type === undefined) return undefined;
  const qualified = (t: any) => {
    const parts = [t.name];
    for (let ns = t.namespace; ns && ns.name !== ""; ns = ns.namespace) parts.unshift(ns.name);
    return parts.join(".");
  };
  switch (type.kind) {
    case "Model":
      return {
        kind: "Model",
        name: qualified(type),
        // TS exposes instance arguments on `templateMapper` (`templateArguments` is deprecated).
        templateArguments: (type.templateMapper?.args ?? type.templateArguments ?? []).map(
          (t: any) => describeType(t, 1),
        ),
        indexer: type.indexer && {
          key: describeType(type.indexer.key, 1),
          value: describeType(type.indexer.value, 1),
        },
        ...(depth === 0
          ? {
              properties: [...type.properties.values()].map((p: any) => ({
                name: p.name,
                optional: p.optional,
                type: describeType(p.type, 1),
              })),
            }
          : {}),
      };
    case "Scalar":
      return {
        kind: "Scalar",
        name: qualified(type),
        baseScalar: type.baseScalar && qualified(type.baseScalar),
      };
    case "Union":
      return {
        kind: "Union",
        variants: [...type.variants.values()].map((v: any) => describeType(v.type, 1)),
      };
    case "Intrinsic":
      return { kind: "Intrinsic", name: type.name };
    case "TemplateParameter":
      return { kind: "TemplateParameter" };
    default:
      return { kind: type.kind, name: type.name };
  }
}

describe("standard library comparison: Rust vs TS", () => {
  const cases: Array<[string, string]> = [
    ["array and record properties", `model A { tags: string[]; map: Record<int32>; }`],
    ["array of models", `model Foo {} model A { foos: Foo[]; nested: string[][] }`],
    ["explicit Array reference", `alias X = Array<string>; model A { x: X; y: Array<int32> }`],
    ["model is Array", `model A is Array<string>;`],
    ["model is Record", `model A is Record<string> { x: string }`],
    ["unknown indexers", `model A { x: unknown[]; y: Record<unknown> }`],
    ["custom scalar array", `scalar myInt extends int32; model A { x: myInt[] }`],
    ["nullable union", `model A { x: string | null }`],
    ["no-array-properties", `model A is string[] { x: int32 }`],
    ["incompatible-indexer", `model A is Record<int32> { x: string }`],
    ["spread record before property", `model A { ...Record<string>; x: int32 }`],
    ["spread into record", `model A is Record<string> { ...B } model B { x: int32 }`],
    ["spread array model", `model B is string[]; model A { ...B }`],
    ["spread multiple records", `model A { ...Record<string>; ...Record<int32>; x: int32 }`],
    [
      "template parameter spread",
      `model Foo<T> { ...T; b: int32 } model X { a: string } model M { f: Foo<X> }`,
    ],
    [
      "reflection constraint satisfied",
      `model Foo<T extends Reflection.Model> { t: T } model M {} model X { a: Foo<M> }`,
    ],
    [
      "reflection constraint violated",
      `model Foo<T extends Reflection.Model> { t: T } model X { a: Foo<string> }`,
    ],
    [
      "template constraint violated",
      `model Foo<T extends string> { value: T } model X { y: Foo<boolean> }`,
    ],
    [
      "named template argument constraint violated",
      `model Foo<T extends string> { value: T } model X { y: Foo<T = boolean> }`,
    ],
    ["invalid template default", `model Foo<T extends string = int32> { value: T }`],
    ["global declaration ambiguous with TypeSpec", `model string {} model A { x: string }`],
    ["explicit using TypeSpec", `using TypeSpec; model A { x: string }`],
    ["qualified TypeSpec reference", `model A { x: TypeSpec.string; y: TypeSpec.Record<int32> }`],
    ["record intersection", `alias I = Record<string> & { x: string }; model A { i: I }`],
    ["array intersection", `alias I = string[] & { x: string }; model A { i: I }`],
    ["decorator argument count", `@doc model A {}`],
    ["augment decorator argument count", `model A {} @@doc(A);`],
    ["decorator argument count in template", `@doc model A<T> { x: T } model B { a: A<string> }`],
    [
      "decorator with valid arguments",
      `@doc("hi") @summary("s") model A { @minValue(1) x: int32 }`,
    ],
    ["unused template parameter", `model A<T> {}`],
    // M12: tuples, string templates and object / array value checking.
    [
      "tuple constraints",
      `model W<T extends string[]> { t: T } model Y<T extends [string, int32]> { t: T }
      model X { pair: [string, int32]; a: W<["a"]>; b: W<[1]>; c: Y<[string]> }`,
    ],
    [
      "string templates",
      `alias P = "pre"; model X { a: "\${P}-x"; b: "\${P}-\${int32}" } @doc("\${P} \${int32}") model D {}`,
    ],
    [
      "value defaults",
      `model P { x: int32; y?: string } const c = #{ x: 1 };
      model X {
        a?: int32[] = #[1, "b"]; b?: P = #{ x: "s" }; c?: P = #{ x: 1, z: 2 };
        d?: P = #{ y: "a" }; h?: int32 = #[1]; r?: Record<int32> = #{ a: "x" };
        u?: string | int32 = #[1]; s?: P = #{ ...c, y: "ok" }; ok?: P = c;
        t?: [int32, string] = #[1]; t2?: [int32, string] = #[1, 2];
        dt?: string = utcDateTime.fromISO("2020-01-01T00:00:00Z");
        n?: { a: { b: int32 } } = #{ a: #{ b: "x" } };
      }`,
    ],
    ["typed const", `model P { x: int32 } const bad: P = #{ x: "s" }; const ok: P = #{ x: 1 };`],
    ["decorator value argument", `@service(#{ title: 1 }) namespace A {}`],
  ];

  it.each(cases)("%s", async (_, source) => {
    const ts = await compileTs(source);
    const rust = compileRust(source);
    expect(rust.diagnostics).toEqual(ts.diagnostics);
    const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name);
    const tsModels = [...ts.program.getGlobalNamespaceType().models.values()].sort(byName);
    const rustModels = [...rust.program.getGlobalNamespaceType().models.values()].sort(byName);
    expect(rustModels.map((m) => describeType(m))).toEqual(tsModels.map((m) => describeType(m)));
  });

  it("loads the standard library from @typespec/compiler", () => {
    const { result } = compileRust(`model A {}`);
    const root = getCompilerRoot();
    expect(result.diagnostics).toEqual([]);
    expect(
      result.sourceFiles.map((x) =>
        x.path.startsWith(root + "/") ? x.path.slice(root.length + 1) : x.path,
      ),
    ).toEqual([
      "lib/intrinsics.tsp",
      "dist/src/lib/intrinsic/tsp-index.js",
      "lib/prototypes.tsp",
      "lib/std/main.tsp",
      "lib/std/types.tsp",
      "lib/std/decorators.tsp",
      "dist/src/lib/tsp-index.js",
      "lib/std/reflection.tsp",
      "lib/std/visibility.tsp",
      mainPath,
    ]);
  });

  it("links intrinsic scalars to their standard library declarations", async () => {
    const source = `model A { s: string; i: int32; f: float64; u: url; d: utcDateTime }`;
    const ts = await compileTs(source);
    const rust = compileRust(source);
    const describeProps = (program: any) =>
      [...program.getGlobalNamespaceType().models.get("A").properties.values()].map((p: any) =>
        describeType(p.type),
      );
    expect(describeProps(rust.program)).toEqual(describeProps(ts.program));
    const typespec = rust.program.getGlobalNamespaceType().namespaces.get("TypeSpec")!;
    const int32 = typespec.scalars.get("int32")!;
    expect(int32.baseScalar?.name).toBe("int64");
    expect(rust.program.getGlobalNamespaceType().models.get("A")!.properties.get("i")!.type).toBe(
      int32,
    );
  });
});
