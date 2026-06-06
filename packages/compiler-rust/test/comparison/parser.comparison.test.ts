import { describe, expect, it } from "vitest";
import { parse as tsParse, visitChildren } from "../../../compiler/src/core/parser.js";
import type { Node } from "../../../compiler/src/core/types.js";
import { parse as rustParse } from "../../index.js";

interface RustNode {
  kind: number;
  pos: number;
  end: number;
  children: number[];
}

interface RustAst {
  root: number;
  nodes: RustNode[];
  diagnostics: { code: string; pos: number; end: number }[];
}

function normalizeTs(node: Node): unknown {
  const children: unknown[] = [];
  visitChildren(node, (child) => {
    children.push(normalizeTs(child));
    return undefined;
  });
  return {
    kind: node.kind,
    pos: node.pos,
    end: node.end,
    children,
  };
}

function normalizeRust(ast: RustAst, id = ast.root): unknown {
  const node = ast.nodes[id];
  return {
    kind: node.kind,
    pos: node.pos,
    end: node.end,
    children: node.children.map((child) => normalizeRust(ast, child)),
  };
}

function compare(source: string) {
  const tsResult = tsParse(source);
  const rustResult = rustParse(source) as RustAst;
  expect(normalizeRust(rustResult)).toEqual(normalizeTs(tsResult));
  expect(rustResult.diagnostics.length).toBe(tsResult.parseDiagnostics.length);
}

describe("parser comparison: Rust vs TS", () => {
  const cases = [
    { name: "empty file", source: "" },
    { name: "empty statement", source: ";" },
    { name: "import", source: 'import "./foo.tsp";' },
    { name: "using", source: "using Foo.Bar;" },
    { name: "single model", source: "model Foo {}" },
    { name: "model primitive properties", source: "model Foo { name: string; age?: int32; }" },
    { name: "model spread property", source: "model Foo { ...Bar; name: string; }" },
    { name: "model extends", source: "model Foo extends Bar { name: string; }" },
    { name: "model is", source: "model Foo is Bar;" },
    { name: "model expression", source: "alias Foo = { name: string; nested: { id: string } };" },
    { name: "array expression", source: "alias Foo = string[][];" },
    { name: "tuple expression", source: "alias Foo = [string, int32, boolean];" },
    { name: "union expression", source: "alias Foo = string | int32 | boolean;" },
    { name: "intersection expression", source: "alias Foo = A & B & C;" },
    { name: "member type reference", source: "alias Foo = A.B.C;" },
    { name: "template reference", source: "alias Foo = Record<string>;" },
    { name: "named template argument", source: "alias Foo = Record<K = string>;" },
    { name: "valueof expression", source: "alias Foo = valueof string;" },
    { name: "typeof expression", source: "alias Foo = typeof foo;" },
    { name: "boolean literal", source: "const foo = true;" },
    { name: "numeric literal", source: "const foo = 123;" },
    { name: "string literal", source: 'const foo = "bar";' },
    { name: "object literal", source: 'const foo = #{ name: "bar", ...other };' },
    { name: "array literal", source: 'const foo = #["bar", 123, true];' },
    { name: "enum", source: "enum Color { red, blue: 2, green: \"green\" }" },
    { name: "enum spread", source: "enum Color { ...Base, red }" },
    { name: "union statement", source: "union Foo { string, named: int32 }" },
    { name: "interface", source: "interface Foo { op read(): string; }" },
    { name: "interface extends", source: "interface Foo extends Bar, Baz { op read(): string; }" },
    { name: "operation", source: "op read(id: string): string;" },
    { name: "operation is", source: "op read is other;" },
    { name: "scalar semicolon", source: "scalar utcDateTime;" },
    { name: "scalar extends", source: "scalar utcDateTime extends string;" },
    { name: "scalar constructor", source: "scalar Foo { init fromString(value: string); }" },
    { name: "namespace block", source: "namespace Foo { model Bar {} }" },
    { name: "namespace dotted", source: "namespace Foo.Bar;" },
    { name: "decorated model", source: "@doc(\"hi\") model Foo {}" },
    { name: "decorated property", source: "model Foo { @doc(\"hi\") name: string; }" },
    { name: "augment decorator", source: "@@doc(Foo, \"hi\");" },
    { name: "decorator declaration", source: "extern dec doc(target: Type, value: valueof string);" },
    { name: "function declaration", source: "extern fn camelCase(value: valueof string): valueof string;" },
    { name: "function type", source: "alias Foo = fn(value: valueof string) => valueof string;" },
    { name: "template declaration", source: "model Foo<T extends string = string> { value: T; }" },
    { name: "void keyword", source: "op ping(): void;" },
    { name: "never keyword", source: "alias Foo = never;" },
    { name: "unknown keyword", source: "alias Foo = unknown;" },
    { name: "reserved property name", source: "model Foo { private: string; }" },
    { name: "missing model brace recovery", source: "model Foo { name: string;" },
  ];

  for (const testCase of cases) {
    it(testCase.name, () => compare(testCase.source));
  }
});
