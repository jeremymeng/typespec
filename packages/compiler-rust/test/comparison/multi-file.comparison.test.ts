import { describe, expect, it } from "vitest";
import { getSourceLocation } from "../../../compiler/src/core/diagnostics.js";
import type { Namespace as TsNamespace } from "../../../compiler/src/core/types.js";
import { mockFile, resolveVirtualPath } from "../../../compiler/src/testing/fs.js";
import { Tester } from "../../../compiler/test/tester.js";
import type { CheckProgramResultJs } from "../../index.js";
import { checkProgram, createVirtualProgramHost } from "../../src/host.js";

interface NormalizedDiagnostic {
  code: string;
  severity: string;
  message: string;
  file?: string;
  pos?: number;
  end?: number;
}

/** Files keyed by path relative to the virtual root. `.js` files are JS stubs. */
type Files = Record<string, string>;

const root = resolveVirtualPath("");

function relative(path: string | undefined): string | undefined {
  if (path === undefined) return undefined;
  const normalized = path.replace(/\\/g, "/");
  return normalized.startsWith(root + "/") ? normalized.slice(root.length + 1) : normalized;
}

function sortDiagnostics(diagnostics: NormalizedDiagnostic[]) {
  return [...diagnostics].sort((a, b) =>
    `${a.file}:${String(a.pos).padStart(6)}:${a.code}`.localeCompare(
      `${b.file}:${String(b.pos).padStart(6)}:${b.code}`,
    ),
  );
}

interface TsOutcome {
  diagnostics: NormalizedDiagnostic[];
  sourceFiles: string[];
  namespaces: Record<string, string[]>;
}

function collectTsNamespaces(ns: TsNamespace, prefix: string, out: Record<string, string[]>) {
  for (const child of ns.namespaces.values()) {
    if (prefix === "" && child.name === "TypeSpec") continue;
    const name = prefix ? `${prefix}.${child.name}` : child.name;
    out[name] = [...child.models.keys()].sort();
    collectTsNamespaces(child, name, out);
  }
}

async function compileTs(files: Files): Promise<TsOutcome> {
  const { "main.tsp": main, ...others } = files;
  const mocks = Object.fromEntries(
    Object.entries(others).map(([path, text]) => [
      path,
      path.endsWith(".js") ? mockFile.js({}) : text,
    ]),
  );
  const [{ program }, diagnostics] = await Tester.files(mocks).compileAndDiagnose(main, {
    // The Rust checker always reports `unused-using`; in TS it is an opt-in linter rule.
    compilerOptions: { linterRuleSet: { enable: { "@typespec/compiler/unused-using": true } } },
  });
  const namespaces: Record<string, string[]> = {
    "": [...program.getGlobalNamespaceType().models.keys()].sort(),
  };
  collectTsNamespaces(program.getGlobalNamespaceType(), "", namespaces);
  const sourceFiles = [...program.sourceFiles.keys(), ...program.jsSourceFiles.keys()]
    .map((x) => relative(x)!)
    .filter((x) => !x.includes("node_modules/@typespec/compiler/") && !x.startsWith(".tsp/"))
    .sort();
  return {
    diagnostics: sortDiagnostics(
      diagnostics.map((d) => {
        const location = getSourceLocation(d.target);
        return {
          code: d.code.replace(/^@typespec\/compiler\//, ""),
          severity: d.severity,
          message: d.message,
          ...(location
            ? { file: relative(location.file.path), pos: location.pos, end: location.end }
            : {}),
        };
      }),
    ),
    sourceFiles,
    namespaces,
  };
}

interface RustType {
  id: number;
  kind: string;
  name: string;
  namespaceId?: number;
  modelNames: string[];
  namespaceIds: number[];
}

function checkRust(files: Files): CheckProgramResultJs {
  const host = createVirtualProgramHost(
    Object.fromEntries(
      Object.entries(files).map(([path, text]) => [resolveVirtualPath(path), text]),
    ),
  );
  return checkProgram(resolveVirtualPath("main.tsp"), host);
}

function compileRust(files: Files): TsOutcome {
  const result = checkRust(files);
  const types = result.types as unknown as RustType[];
  const byId = new Map(types.map((t) => [t.id, t]));
  const global = types.find((t) => t.kind === "Namespace" && t.name === "")!;
  const namespaces: Record<string, string[]> = { "": [...global.modelNames].sort() };
  const visit = (ns: RustType, prefix: string) => {
    for (const id of ns.namespaceIds) {
      const child = byId.get(id)!;
      if (prefix === "" && child.name === "TypeSpec") continue;
      const name = prefix ? `${prefix}.${child.name}` : child.name;
      namespaces[name] = [...child.modelNames].sort();
      visit(child, name);
    }
  };
  visit(global, "");
  // TS skips the linter stage (where `unused-using` is reported) once an error
  // has been reported; the Rust checker has no linter stage and always reports it.
  const hasError = result.diagnostics.some((d) => d.severity === "error");
  const diagnostics = result.diagnostics.filter((d) => !(hasError && d.code === "unused-using"));
  return {
    diagnostics: sortDiagnostics(
      diagnostics.map((d) => ({
        code: d.code,
        severity: d.severity,
        message: d.message,
        ...(d.file !== undefined ? { file: relative(d.file), pos: d.pos, end: d.end } : {}),
      })),
    ),
    sourceFiles: result.sourceFiles.map((x) => relative(x.path)!).sort(),
    namespaces,
  };
}

async function expectParity(files: Files): Promise<TsOutcome> {
  const ts = await compileTs(files);
  const rust = compileRust(files);
  expect(rust.diagnostics).toEqual(ts.diagnostics);
  expect(rust.sourceFiles).toEqual(ts.sourceFiles);
  expect(rust.namespaces).toEqual(ts.namespaces);
  return rust;
}

const pkg = (json: object) => JSON.stringify(json);

describe("multi-file program comparison: Rust vs TS", () => {
  const cases: Array<[string, Files]> = [
    [
      "two files",
      { "main.tsp": `import "./other.tsp"; model A { b: B }`, "other.tsp": `model B {}` },
    ],
    [
      "circular imports",
      {
        "main.tsp": `import "./a.tsp"; model M { a: A }`,
        "a.tsp": `import "./b.tsp"; model A { b: B }`,
        "b.tsp": `import "./a.tsp"; import "./main.tsp"; model B { m: M }`,
      },
    ],
    [
      "diamond imports",
      {
        "main.tsp": `import "./a.tsp"; import "./b.tsp";`,
        "a.tsp": `import "./c.tsp"; model A { c: C }`,
        "b.tsp": `import "./c.tsp"; model B { c: C }`,
        "c.tsp": `model C {}`,
      },
    ],
    [
      "nested directories and parent paths",
      {
        "main.tsp": `import "./sub/a.tsp";`,
        "sub/a.tsp": `import "../shared/b.tsp"; import "./deeper/c.tsp"; model A { b: B; c: C }`,
        "shared/b.tsp": `model B {}`,
        "sub/deeper/c.tsp": `model C {}`,
      },
    ],
    [
      "block namespace merge across files",
      {
        "main.tsp": `import "./other.tsp"; namespace N { model A { b: B } }`,
        "other.tsp": `namespace N { model B {} }`,
      },
    ],
    [
      "blockless namespace merge across files",
      {
        "main.tsp": `import "./other.tsp"; namespace N; model A { b: B }`,
        "other.tsp": `namespace N; model B { a: A }`,
      },
    ],
    [
      "dotted blockless namespace merge",
      {
        "main.tsp": `import "./other.tsp"; namespace A.B; model X { y: Y }`,
        "other.tsp": `namespace A.B; model Y {} namespace C { model Z {} }`,
      },
    ],
    [
      "dotted and nested namespaces merge",
      {
        "main.tsp": `import "./other.tsp"; namespace A.B { model X {} }`,
        "other.tsp": `namespace A { namespace B { model Y {} } }`,
      },
    ],
    [
      "fully qualified cross-file reference",
      {
        "main.tsp": `import "./other.tsp"; model A { b: Lib.B }`,
        "other.tsp": `namespace Lib; model B {}`,
      },
    ],
    ["missing relative import", { "main.tsp": `import "./missing.tsp"; model A {}` }],
    ["missing package import", { "main.tsp": `import "missing-lib"; model A {}` }],
    [
      "import of directory with main.tsp",
      { "main.tsp": `import "./lib"; model A { b: B }`, "lib/main.tsp": `model B {}` },
    ],
    [
      "import of directory without entrypoint",
      { "main.tsp": `import "./lib"; model A {}`, "lib/other.tsp": `model B {}` },
    ],
    ["self import", { "main.tsp": `import "./main.tsp"; model A {}` }],
    [
      "duplicate import",
      { "main.tsp": `import "./b.tsp"; import "./b.tsp"; model A {}`, "b.tsp": `model B {}` },
    ],
    [
      "invalid import extension",
      { "main.tsp": `import "./data.json"; model A {}`, "data.json": `{}` },
    ],
    ["js import is recorded", { "main.tsp": `import "./lib.js"; model A {}`, "lib.js": `` }],
    [
      "package with tspMain",
      {
        "main.tsp": `import "my-lib"; model A { b: B }`,
        "node_modules/my-lib/package.json": pkg({ name: "my-lib", tspMain: "lib/main.tsp" }),
        "node_modules/my-lib/lib/main.tsp": `import "./b.tsp";`,
        "node_modules/my-lib/lib/b.tsp": `model B {}`,
      },
    ],
    [
      "package with js main and tsp exports condition",
      {
        "main.tsp": `import "my-lib"; model A { b: Lib.B }`,
        "node_modules/my-lib/package.json": pkg({
          name: "my-lib",
          main: "dist/index.js",
          exports: { ".": { typespec: "./lib/main.tsp", default: "./dist/index.js" } },
        }),
        "node_modules/my-lib/lib/main.tsp": `namespace Lib; model B {}`,
        "node_modules/my-lib/dist/index.js": ``,
      },
    ],
    [
      "package with js main only",
      {
        "main.tsp": `import "my-lib"; model A {}`,
        "node_modules/my-lib/package.json": pkg({ name: "my-lib", main: "index.js" }),
        "node_modules/my-lib/index.js": ``,
      },
    ],
    [
      "scoped package subpath export",
      {
        "main.tsp": `import "@scope/lib/extra"; model A { e: E }`,
        "node_modules/@scope/lib/package.json": pkg({
          name: "@scope/lib",
          exports: {
            ".": { typespec: "./main.tsp" },
            "./extra": { typespec: "./extra.tsp" },
          },
        }),
        "node_modules/@scope/lib/main.tsp": `model M {}`,
        "node_modules/@scope/lib/extra.tsp": `model E {}`,
      },
    ],
    [
      "package missing main",
      {
        "main.tsp": `import "bad-lib"; model A {}`,
        "node_modules/bad-lib/package.json": pkg({ name: "bad-lib" }),
      },
    ],
    [
      "package main pointing nowhere",
      {
        "main.tsp": `import "bad-lib"; model A {}`,
        "node_modules/bad-lib/package.json": pkg({ name: "bad-lib", main: "missing.js" }),
      },
    ],
    [
      "package with invalid export target",
      {
        "main.tsp": `import "bad-lib"; model A {}`,
        "node_modules/bad-lib/package.json": pkg({
          name: "bad-lib",
          exports: { ".": { typespec: "lib/main.tsp" } },
        }),
        "node_modules/bad-lib/lib/main.tsp": `model B {}`,
      },
    ],
    [
      "package subpath not exported",
      {
        "main.tsp": `import "my-lib/hidden.tsp"; model A {}`,
        "node_modules/my-lib/package.json": pkg({
          name: "my-lib",
          exports: { ".": { typespec: "./main.tsp" } },
        }),
        "node_modules/my-lib/main.tsp": ``,
        "node_modules/my-lib/hidden.tsp": ``,
      },
    ],
    [
      "duplicate symbol across files",
      { "main.tsp": `import "./other.tsp"; model A {}`, "other.tsp": `model A {}` },
    ],
    [
      "parse error in imported file",
      { "main.tsp": `import "./other.tsp"; model A {}`, "other.tsp": `model B {` },
    ],
    [
      "unknown identifier in imported file",
      { "main.tsp": `import "./other.tsp"; model A {}`, "other.tsp": `model B { x: Missing }` },
    ],
    [
      "using is scoped to its file",
      {
        "main.tsp": `import "./other.tsp"; namespace Lib { model L {} } using Lib; model A { l: L }`,
        "other.tsp": `model B { l: L }`,
      },
    ],
    [
      "unused using in imported file",
      {
        "main.tsp": `import "./other.tsp"; namespace Lib { model L {} }`,
        "other.tsp": `using Lib; model B {}`,
      },
    ],
    [
      "unused using in a library file",
      {
        "main.tsp": `import "my-lib"; namespace Lib { model L {} }`,
        "node_modules/my-lib/package.json": pkg({ name: "my-lib", tspMain: "main.tsp" }),
        "node_modules/my-lib/main.tsp": `using Lib; model B {}`,
      },
    ],
    [
      "using in imported file resolves names there",
      {
        "main.tsp": `import "./other.tsp"; namespace Lib { model L {} }`,
        "other.tsp": `using Lib; model B { l: L }`,
      },
    ],
    [
      // upstream 325c16580
      "using before file namespace resolves from the global namespace",
      {
        "main.tsp": `import "./other.tsp"; using Lib; namespace App; model A { l: L }`,
        "other.tsp": `namespace Lib { model L {} } namespace App.Lib { model Other {} }`,
      },
    ],
    [
      "using after blockless namespace resolves relative to it",
      {
        "main.tsp": `import "./other.tsp"; namespace App; using Lib; model A { o: Other }`,
        "other.tsp": `namespace Lib { model L {} } namespace App.Lib { model Other {} }`,
      },
    ],
    [
      "global declaration and using are ambiguous",
      {
        "main.tsp": `import "./other.tsp"; using Lib; model A { x: X }`,
        "other.tsp": `model X {} namespace Lib { model X {} }`,
      },
    ],
  ];

  const orderingCases: Array<[string, Files]> = [
    ["import after declaration", { "main.tsp": `model A {}\nimport "./b.tsp";`, "b.tsp": `` }],
    ["import after using", { "main.tsp": `using TypeSpec;\nimport "./b.tsp";`, "b.tsp": `` }],
    [
      "import after blockless namespace",
      { "main.tsp": `namespace N;\nimport "./b.tsp";`, "b.tsp": `` },
    ],
    ["import inside namespace", { "main.tsp": `namespace N { import "./b.tsp"; }`, "b.tsp": `` }],
    ["multiple blockless namespaces", { "main.tsp": `namespace A;\nnamespace B;\nmodel M {}` }],
    ["blockless namespace after declaration", { "main.tsp": `model M {}\nnamespace A;` }],
    ["blockless namespace inside block", { "main.tsp": `namespace A { namespace B; }` }],
    [
      "extensionless relative import does not add .tsp",
      { "main.tsp": `import "./other"; model A {}`, "other.tsp": `model B {}` },
    ],
    [
      "relative import of a package directory is a library",
      {
        "main.tsp": `import "./vendor"; namespace Lib { model L {} }`,
        "vendor/package.json": pkg({ name: "vendor", tspMain: "main.tsp" }),
        "vendor/main.tsp": `import "./inner.tsp"; using Lib;`,
        "vendor/inner.tsp": `using Lib;`,
      },
    ],
    [
      "package resolves itself by name",
      {
        "main.tsp": `import "self-pkg/extra"; model A { e: E }`,
        "package.json": pkg({
          name: "self-pkg",
          exports: { "./extra": { typespec: "./extra.tsp" } },
        }),
        "extra.tsp": `model E {}`,
      },
    ],
  ];

  it.each(orderingCases)("%s", async (_name, files) => {
    await expectParity(files);
  });

  it.each(cases)("%s", async (_name, files) => {
    await expectParity(files);
  });

  it("reports an import-not-found with a location in the importing file", async () => {
    const rust = await expectParity({ "main.tsp": `model A {}\nimport "./missing.tsp";` });
    expect(rust.diagnostics.map((d) => d.code)).toContain("import-not-found");
  });

  it("checks a directory entrypoint", () => {
    const host = createVirtualProgramHost({
      [resolveVirtualPath("proj/main.tsp")]: `import "./b.tsp"; model A { b: B }`,
      [resolveVirtualPath("proj/b.tsp")]: `model B {}`,
    });
    const result = checkProgram(resolveVirtualPath("proj"), host);
    expect(result.diagnostics).toEqual([]);
    expect(result.sourceFiles.map((x) => relative(x.path))).toEqual([
      "proj/main.tsp",
      "proj/b.tsp",
    ]);
  });
});
