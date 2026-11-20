import { describe, expect, it } from "vitest";
import { SyntaxKind } from "../../../compiler/src/core/types.js";
import { Tester } from "../../../compiler/test/tester.js";
import { bind, parse } from "../../index.js";

interface RustSymbol {
  id: number;
  name: string;
  flags: number;
  declarationNodeIds: number[];
  parentScopeId: number;
  membersScopeId?: number;
}

interface RustScope {
  id: number;
  kind: string;
  parentId?: number;
  owningNodeId?: number;
  localSymbolIds: number[];
  localSymbolNames: string[];
  usingScopeIds: number[];
}

interface RustResolution {
  nodeId: number;
  kind: "resolved" | "ambiguous" | "unknown";
  symbolId?: number;
  ambiguousSymbolIds: number[];
}

interface RustBindResult {
  symbols: RustSymbol[];
  scopes: RustScope[];
  resolutions: RustResolution[];
  globalScopeId: number;
  diagnostics: { severity: string; code: string; message: string; pos: number; end: number }[];
}

interface RustNode {
  kind: number;
  pos: number;
  end: number;
  sv?: string;
  target?: number;
  base?: number;
  id?: number;
  valueNode?: number;
  name?: number;
  members?: number[];
  statements?: number[];
  properties?: number[];
  operations?: number[];
  options?: number[];
}

interface RustAst {
  root: number;
  nodes: RustNode[];
}

const Flags = {
  Model: 1 << 1,
  Scalar: 1 << 2,
  Operation: 1 << 3,
  Enum: 1 << 4,
  Interface: 1 << 5,
  Union: 1 << 6,
  Alias: 1 << 7,
  Namespace: 1 << 8,
  Decorator: 1 << 9,
  TemplateParameter: 1 << 10,
  Function: 1 << 11,
  FunctionParameter: 1 << 12,
  Member: 1 << 16,
  Const: 1 << 17,
  Declaration: 1 << 20,
};

function rust(source: string): RustBindResult {
  return bind(source) as RustBindResult;
}

function ast(source: string): RustAst {
  return parse(source) as RustAst;
}

function declaredSymbols(result: RustBindResult) {
  return result.symbols.filter((x) => x.declarationNodeIds.length > 0);
}

function symbol(result: RustBindResult, name: string): RustSymbol {
  const found = declaredSymbols(result).find((x) => x.name === name);
  if (found === undefined) {
    throw new Error(`Missing symbol ${name}. Found: ${declaredSymbols(result).map((x) => x.name)}`);
  }
  return found;
}

function expectSymbol(source: string, name: string, flag: number) {
  const result = rust(source);
  const sym = symbol(result, name);
  expect(sym.flags & flag).toBe(flag);
  expect(sym.flags & Flags.Declaration).toBe(Flags.Declaration);
  expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
}

function memberNames(result: RustBindResult, ownerName: string): string[] {
  const owner = symbol(result, ownerName);
  expect(owner.membersScopeId).toBeDefined();
  return result.scopes[owner.membersScopeId!].localSymbolNames;
}

function typeReferenceForTarget(source: string, targetName: string, occurrence = 0): number {
  const parsed = ast(source);
  const matches = parsed.nodes
    .map((node, id) => ({ ...node, nodeId: id }))
    .filter((node) => {
      if (node.kind !== SyntaxKind.TypeReference || node.target === undefined) return false;
      return expressionToString(parsed, node.target) === targetName;
    });
  expect(matches.length).toBeGreaterThan(occurrence);
  return matches[occurrence].nodeId;
}

function expressionToString(parsed: RustAst, nodeId: number): string {
  const node = parsed.nodes[nodeId];
  if (node.sv !== undefined) return node.sv;
  if (node.kind === SyntaxKind.TypeReference && node.target !== undefined) {
    return expressionToString(parsed, node.target);
  }
  if (
    node.kind === SyntaxKind.MemberExpression &&
    node.id !== undefined &&
    node.base !== undefined
  ) {
    return `${expressionToString(parsed, node.base)}.${expressionToString(parsed, node.id)}`;
  }
  return "<unknown>";
}

function resolutionFor(result: RustBindResult, nodeId: number): RustResolution {
  const resolution = result.resolutions.find((x) => x.nodeId === nodeId);
  if (resolution === undefined) {
    throw new Error(`No resolution for node ${nodeId}`);
  }
  return resolution;
}

function expectResolvedTo(source: string, targetName: string, symbolName: string, occurrence = 0) {
  const result = rust(source);
  const nodeId = typeReferenceForTarget(source, targetName, occurrence);
  const resolution = resolutionFor(result, nodeId);
  expect(resolution.kind).toBe("resolved");
  expect(result.symbols[resolution.symbolId!].name).toBe(symbolName);
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

describe("binder comparison: Rust vs TS patterns", () => {
  describe("symbol creation", () => {
    const cases = [
      ["model", "model Foo {}", "Foo", Flags.Model],
      ["enum", "enum Color { red }", "Color", Flags.Enum],
      ["union", "union Choice { a: string }", "Choice", Flags.Union],
      ["scalar", "scalar uuid extends string;", "uuid", Flags.Scalar],
      ["alias", "alias Foo = string;", "Foo", Flags.Alias],
      ["operation", "op read(): string;", "read", Flags.Operation],
      ["interface", "interface Store { op read(): string; }", "Store", Flags.Interface],
      ["namespace", "namespace Store { model Read {} }", "Store", Flags.Namespace],
      ["const", 'const name: string = "abc";', "name", Flags.Const],
      [
        "decorator declaration",
        "extern dec doc(target: Type, value: valueof string);",
        "@doc",
        Flags.Decorator,
      ],
      [
        "function declaration",
        "extern fn camel(value: valueof string): valueof string;",
        "camel",
        Flags.Function,
      ],
      ["template parameter", "model Box<T> { value: T; }", "T", Flags.TemplateParameter],
    ] as const;

    for (const [name, source, symbolName, flag] of cases) {
      it(name, () => expectSymbol(source, symbolName, flag));
    }
  });

  describe("member scopes", () => {
    const cases = [
      ["model properties", "model Foo { x: string; y: int32; }", "Foo", ["x", "y"]],
      ["enum members", "enum Color { red, blue }", "Color", ["blue", "red"]],
      [
        "interface operations",
        "interface Store { op read(): string; op list(): string[]; }",
        "Store",
        ["list", "read"],
      ],
      ["union variants", "union Choice { a: string; b: int32; }", "Choice", ["a", "b"]],
      [
        "scalar constructors",
        "scalar Foo { init fromString(value: string); }",
        "Foo",
        ["fromString"],
      ],
    ] as const;

    for (const [name, source, owner, expected] of cases) {
      it(name, () => {
        expect(memberNames(rust(source), owner).sort()).toEqual([...expected].sort());
      });
    }

    it("nests namespace scopes", () => {
      const result = rust("namespace A { namespace B { model C {} } }");
      const a = symbol(result, "A");
      const b = symbol(result, "B");
      const c = symbol(result, "C");
      expect(result.scopes[b.parentScopeId].owningNodeId).toBe(a.declarationNodeIds[0]);
      expect(result.scopes[c.parentScopeId].owningNodeId).toBe(b.declarationNodeIds[0]);
    });
  });

  describe("name resolution", () => {
    const cases = [
      ["intrinsic string", "model Foo { name: string; }", "string", "string", 0],
      ["intrinsic int32", "model Foo { age: int32; }", "int32", "int32", 0],
      ["simple model ref", "model Bar {}\nmodel Foo { bar: Bar; }", "Bar", "Bar", 0],
      ["alias target", "model Bar {}\nalias Foo = Bar;", "Bar", "Bar", 0],
      ["operation return", "model Bar {}\nop read(): Bar;", "Bar", "Bar", 0],
      ["operation parameter", "model Id {}\nop read(id: Id): string;", "Id", "Id", 0],
      [
        "interface operation return",
        "model Bar {}\ninterface I { op read(): Bar; }",
        "Bar",
        "Bar",
        0,
      ],
      ["template parameter", "model Box<T> { value: T; }", "T", "T", 0],
      ["model extends", "model Base {}\nmodel Foo extends Base {}", "Base", "Base", 0],
      ["model is", "model Base {}\nmodel Foo is Base;", "Base", "Base", 0],
      ["scalar extends", "scalar Foo extends string;", "string", "string", 0],
      [
        "qualified namespace",
        "namespace A { model Foo {} }\nalias Bar = A.Foo;",
        "A.Foo",
        "Foo",
        0,
      ],
    ] as const;

    for (const [name, source, target, resolved, occurrence] of cases) {
      it(name, () => expectResolvedTo(source, target, resolved, occurrence));
    }

    it("resolves through using", () => {
      expectResolvedTo("namespace A { model Foo {} }\nusing A;\nalias Bar = Foo;", "Foo", "Foo", 0);
    });

    it("reports ambiguous using matches", () => {
      const source =
        "namespace A { model Foo {} }\nnamespace B { model Foo {} }\nusing A;\nusing B;\nalias Bar = Foo;";
      const result = rust(source);
      const resolution = resolutionFor(result, typeReferenceForTarget(source, "Foo", 0));
      expect(resolution.kind).toBe("ambiguous");
      expect(resolution.ambiguousSymbolIds.map((id) => result.symbols[id].name)).toEqual([
        "Foo",
        "Foo",
      ]);
    });
  });

  describe("diagnostics compared with full TS checker", () => {
    const cases = [
      ["duplicate model", "model Foo {}\nmodel Foo {}", ["duplicate-symbol"]],
      ["unknown type", "alias Foo = Missing;", ["invalid-ref"]],
      ["unknown property type", "model Foo { x: Missing; }", ["invalid-ref"]],
    ] as const;

    for (const [name, source, codes] of cases) {
      it(name, async () => expectDiagnosticsLikeTs(source, [...codes]), 20_000);
    }
  });

  describe("duplicate member diagnostics", () => {
    const cases = [
      ["duplicate enum member", "enum Color { red, red }", "red"],
      ["duplicate property", "model Foo { x: string; x: int32; }", "x"],
      [
        "duplicate interface operation",
        "interface I { op read(): string; op read(): string; }",
        "read",
      ],
    ] as const;

    for (const [name, source, duplicateName] of cases) {
      it(name, () => {
        const diagnostics = rust(source).diagnostics.filter((x) => x.code === "duplicate-symbol");
        expect(diagnostics).toHaveLength(2);
        expect(diagnostics.map((x) => x.message)).toEqual([
          `Duplicate name: "${duplicateName}"`,
          `Duplicate name: "${duplicateName}"`,
        ]);
      });
    }
  });

  describe("using and augment decorator handling", () => {
    it("records using scope ids", () => {
      const result = rust("namespace A { model Foo {} }\nusing A;\nalias Bar = Foo;");
      // File-level usings live on the per-file scope, not the global scope (TS script locals).
      const fileScopes = result.scopes.filter((x) => x.kind === "File");
      expect(fileScopes).toHaveLength(1);
      expect(fileScopes[0].usingScopeIds.length).toBe(1);
      expect(result.scopes[result.globalScopeId].usingScopeIds).toEqual([]);
    });

    it("reports unused using", () => {
      const diagnostics = rust("namespace A { model Foo {} }\nusing A;").diagnostics;
      expect(diagnostics).toContainEqual(
        expect.objectContaining({
          code: "unused-using",
          message: "'using A' is declared but never used.",
        }),
      );
    });

    it("resolves augment decorator target", () => {
      const result = rust(
        'extern dec doc(target: Type, value: valueof string);\nmodel Foo {}\n@@doc(Foo, "hi");',
      );
      expect(result.diagnostics.filter((x) => x.severity === "error")).toEqual([]);
      const fooTarget = typeReferenceForTarget(
        'extern dec doc(target: Type, value: valueof string);\nmodel Foo {}\n@@doc(Foo, "hi");',
        "Foo",
        0,
      );
      expect(result.symbols[resolutionFor(result, fooTarget).symbolId!].name).toBe("Foo");
    });

    it("reports missing augment target", () => {
      const diagnostics = rust(
        'extern dec doc(target: Type, value: valueof string);\n@@doc(Missing, "hi");',
      ).diagnostics;
      // TS reports only the unresolved reference, not augment-decorator-target.
      expect(diagnostics.map((x) => x.code)).toEqual(["invalid-ref"]);
    });
  });
});
