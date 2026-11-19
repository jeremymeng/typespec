import { describe, expect, it } from "vitest";
import { parse as tsParse, visitChildren } from "../../../compiler/src/core/parser.js";
import { SyntaxKind, type Node, type ParseOptions } from "../../../compiler/src/core/types.js";
import { parse as rustParse } from "../../index.js";

interface RustNode {
  kind: number;
  pos: number;
  end: number;
  children: number[];
  sv?: string;
  value?: string;
  target?: number;
  name?: number;
  values?: number[];
  arguments?: number[];
  docs?: number[];
  parsedAsDocs?: boolean;
}

interface RustAst {
  root: number;
  nodes: RustNode[];
  diagnostics: { code: string; message: string; pos: number; end: number }[];
  comments: number[];
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

function compare(source: string, options?: ParseOptions) {
  const tsResult = tsParse(source, options);
  const rustResult = rustParse(source, options) as RustAst;
  expect(normalizeRust(rustResult)).toEqual(normalizeTs(tsResult));
  expect(rustResult.diagnostics.length).toBe(tsResult.parseDiagnostics.length);
}

function diagnostics(source: string) {
  const tsResult = tsParse(source);
  const rustResult = rustParse(source) as RustAst;
  expect(rustResult.diagnostics).toMatchObject(
    tsResult.parseDiagnostics.map((diag) => ({
      code: diag.code,
      message: diag.message,
      pos: diag.target.pos,
      end: diag.target.end,
    })),
  );
}

function statement(ast: RustAst, index = 0): RustNode {
  return ast.nodes[ast.nodes[ast.root].children[1 + index]];
}

function rustDocSummary(ast: RustAst, node: RustNode) {
  return (node.docs ?? []).map((docId) => {
    const doc = ast.nodes[docId];
    return {
      content: (doc.values ?? []).map((id) => ast.nodes[id].value),
      tags: (doc.arguments ?? []).map((tagId) => {
        const tag = ast.nodes[tagId];
        return {
          kind: tag.kind,
          tagName: ast.nodes[tag.target!].sv,
          name: tag.name === undefined ? undefined : ast.nodes[tag.name].sv,
          content: (tag.values ?? []).map((id) => ast.nodes[id].value),
        };
      }),
    };
  });
}

function tsDocSummary(node: any) {
  return (node.docs ?? []).map((doc: any) => ({
    content: doc.content.map((x: any) => x.text),
    tags: doc.tags.map((tag: any) => ({
      kind: tag.kind,
      tagName: tag.tagName.sv,
      name: tag.paramName?.sv ?? tag.propName?.sv,
      content: tag.content.map((x: any) => x.text),
    })),
  }));
}

function compareDocs(source: string) {
  const tsResult = tsParse(source, { docs: true });
  const rustResult = rustParse(source, { docs: true }) as RustAst;
  expect(rustDocSummary(rustResult, statement(rustResult))).toEqual(
    tsDocSummary(tsResult.statements[0]),
  );
}

function compareComments(source: string, options: ParseOptions = { comments: true }) {
  const tsResult = tsParse(source, options);
  const rustResult = rustParse(source, options) as RustAst;
  expect(
    rustResult.comments.map((id) => {
      const comment = rustResult.nodes[id];
      return {
        kind: comment.kind,
        pos: comment.pos,
        end: comment.end,
        parsedAsDocs: comment.parsedAsDocs,
      };
    }),
  ).toEqual(
    tsResult.comments.map((comment: any) => ({
      kind: comment.kind,
      pos: comment.pos,
      end: comment.end,
      parsedAsDocs: comment.parsedAsDocs,
    })),
  );
}

function findFirstRust(ast: RustAst, kind: SyntaxKind, id = ast.root): RustNode | undefined {
  const node = ast.nodes[id];
  if (node.kind === kind) return node;
  for (const child of node.children) {
    const found = findFirstRust(ast, kind, child);
    if (found) return found;
  }
  return undefined;
}

function findFirstTs(node: any, kind: SyntaxKind): any {
  if (node.kind === kind) return node;
  let found: any;
  visitChildren(node, (child) => {
    found ??= findFirstTs(child, kind);
    return undefined;
  });
  return found;
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
    { name: "enum", source: 'enum Color { red, blue: 2, green: "green" }' },
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
    { name: "decorated model", source: '@doc("hi") model Foo {}' },
    { name: "decorated property", source: 'model Foo { @doc("hi") name: string; }' },
    { name: "augment decorator", source: '@@doc(Foo, "hi");' },
    {
      name: "decorator declaration",
      source: "extern dec doc(target: Type, value: valueof string);",
    },
    {
      name: "function declaration",
      source: "extern fn camelCase(value: valueof string): valueof string;",
    },
    { name: "function type", source: "alias Foo = fn(value: valueof string) => valueof string;" },
    { name: "template declaration", source: "model Foo<T extends string = string> { value: T; }" },
    { name: "void keyword", source: "op ping(): void;" },
    { name: "never keyword", source: "alias Foo = never;" },
    { name: "unknown keyword", source: "alias Foo = unknown;" },
    { name: "reserved property name", source: "model Foo { private: string; }" },
    { name: "missing model brace recovery", source: "model Foo { name: string;" },
    { name: "second import form", source: 'import "x";' },
    { name: "multiple usings", source: "using Foo; using Foo.Bar;" },
    { name: "model empty with semicolon", source: "model Car { };" },
    {
      name: "model trailing comma properties",
      source: "model Car { prop1: string, prop2: int32, }",
    },
    { name: "model string literal property", source: 'model Car { "prop-1": string; }' },
    { name: "model emoji string property", source: 'model Car { "😀": string; }' },
    { name: "model default property", source: 'model Car { name?: string = "abc"; }' },
    { name: "model multiple spreads", source: "model Car { ...A; ...B; c: string; }" },
    { name: "model member spread template", source: "model Car { ...A.B<C>; }" },
    { name: "model template two params", source: "model Foo<A, B> { a: A; b: B; }" },
    {
      name: "model template constraints",
      source: "model Foo<T extends valueof string> { value: T; }",
    },
    { name: "model nested arrays", source: "model A { foo: B[][]; }" },
    { name: "model union property", source: "model A { foo: B | C; }" },
    { name: "model intersection property", source: "model A { foo: B & C; }" },
    { name: "model parenthesized property", source: "model A { foo: ((B | C) & D)[]; }" },
    { name: "model is with body", source: "model Foo is Bar { extra: string; }" },
    { name: "scalar decorated", source: '@doc("hi") scalar uuid extends string;' },
    {
      name: "scalar constructor multiple params",
      source: "scalar Foo { init fromString(value: string, radix: int32); }",
    },
    {
      name: "scalar constructor trailing comma",
      source: "scalar Foo { init fromString(value: string,); }",
    },
    { name: "operation trailing comma", source: "op read(id: string, filter: string,): string;" },
    { name: "operation template", source: "op read<T>(id: T): T;" },
    { name: "operation model expression params", source: "op read(...Request): Response;" },
    { name: "interface empty", source: "interface Foo {}" },
    { name: "interface templated", source: "interface Foo<T> { op read(): T; }" },
    { name: "interface bare op", source: "interface Foo { read(): string; }" },
    {
      name: "interface multiple ops",
      source: "interface Foo { op read(): string; op list(): string[]; }",
    },
    {
      name: "interface extends templated",
      source: "interface Foo<T> extends Bar<T>, Baz { op read(): T; }",
    },
    { name: "tuple empty", source: "alias EmptyTuple = [];" },
    { name: "tuple trailing comma", source: "alias TrailingTuple = [string, int32,];" },
    { name: "array literal nested", source: 'const foo = #["abc", 123, #{ nested: true }];' },
    { name: "object literal multiple properties", source: 'const foo = #{ a: "abc", b: "def" };' },
    {
      name: "object literal nested",
      source: "const foo = #{ nested: #{ ok: true }, values: #[1, 2] };",
    },
    { name: "call expression no args", source: "const foo = utcDateTime.now();" },
    {
      name: "call expression multiple args",
      source: 'const foo = utcDateTime.fromISO("abc", "def");',
    },
    { name: "typeof string literal", source: 'const foo: typeof "abc" = "abc";' },
    { name: "typeof nested", source: "const foo: typeof other.value = other.value;" },
    { name: "valueof array", source: "alias Foo = valueof string[];" },
    { name: "union leading bar", source: "alias Foo = | string | int32;" },
    { name: "intersection leading ampersand", source: "alias Foo = & A & B;" },
    {
      name: "namespace nested block",
      source: "namespace Store { namespace Read { op get(): string; } }",
    },
    { name: "namespace dotted block", source: "namespace Store.Read { op get(): string; }" },
    { name: "namespace using", source: "namespace Store { using Foo.Bar; model Read {} }" },
    { name: "decorated namespace", source: "@service namespace Store { op get(): string; }" },
    { name: "decorator member target", source: '@foo.bar("hi") model Foo {}' },
    { name: "decorator call values", source: '@foo(#{ a: "abc" }, #[1, 2]) model Foo {}' },
    { name: "augment decorator multiple args", source: '@@doc(Foo, "hi", "there");' },
    { name: "enum empty", source: "enum Foo {}" },
    { name: "enum string and numeric values", source: 'enum Foo { a: "hi", b: 10 }' },
    { name: "enum decorated members", source: "@foo enum Foo { @bar a, @baz b: 10 }" },
    { name: "union bare variants", source: "union Foo { string, int32 }" },
    {
      name: "union decorated named variants",
      source: "@myDec union Foo { @myDec a: string; b: int32; }",
    },
    { name: "alias numeric", source: "alias X = 1;" },
    { name: "alias maybe undefined", source: "alias MaybeUndefined<T> = T | undefined;" },
    { name: "decorator declaration simple", source: "dec myDec(target: Type);" },
    {
      name: "decorator declaration optional",
      source: "extern dec myDec(target: Type, optional?: StringLiteral);",
    },
    {
      name: "decorator declaration rest",
      source: "extern dec myDec(target: Type, ...rest: StringLiteral[]);",
    },
    { name: "function declaration void", source: "fn myFn(): void;" },
    { name: "function declaration no return", source: "extern fn myFn(arg1: StringLiteral);" },
    {
      name: "function declaration optional",
      source: "extern fn myFn(optional?: StringLiteral): void;",
    },
    {
      name: "function declaration rest",
      source: "extern fn myFn(...rest: StringLiteral[]): void;",
    },
    { name: "string template two spans", source: 'alias T = "Start ${"one"} middle ${23} end";' },
    {
      name: "string template escaped interpolation",
      source: 'alias T = "Start ${"one"} middle \\${23} end";',
    },
    { name: "nested string templates", source: 'alias T = "Start ${"nested ${"hi"}"} end";' },
    { name: "triple quoted string", source: 'alias T = """\n  hello\n  """;' },
    {
      name: "triple quoted string escaped interpolation",
      source: 'alias T = """\n  hello \\${name}\n  """;',
    },
    { name: "auto modifier", source: "auto dec myDec(target: Type);" },
    { name: "typeref as identifier", source: "model typeref { typeref: string; }" },
    { name: "union extends", source: 'union Foo extends string { a: "a", b: "b" }' },
    { name: "templated union extends", source: "union Foo<T> extends T { a: T }" },
    { name: "model declaration expression", source: "model Foo { x: model { a: string }; }" },
    {
      name: "named model declaration expression",
      source: "model Foo { x: model Bar extends Base { a: string }; }",
    },
    { name: "model declaration expression is", source: "alias A = model is Base;" },
    { name: "scalar declaration expression", source: "model Foo { x: scalar extends string; }" },
    {
      name: "scalar declaration expression with body",
      source: "alias A = scalar S extends string { init fromS(v: string); };",
    },
    { name: "enum declaration expression", source: 'model Foo { x: enum { a, b: "b" }; }' },
    {
      name: "union declaration expression",
      source: "model Foo { x: union { a: string, int32 }; }",
    },
    {
      name: "union declaration expression extends",
      source: 'alias A = union extends string { "a" };',
    },
    { name: "decorated declaration expression", source: 'model Foo { x: @doc("d") enum { a }; }' },
    {
      name: "doc on declaration expression",
      source: "model Foo { x: /** doc */ model { a: string }; }",
    },
    {
      name: "declaration expressions in template arguments",
      source: "alias A = Foo<model { a: string }, enum { b }>;",
    },
    {
      name: "declaration expressions in tuple",
      source: "alias A = [model { a: string }, union { b: int32 }];",
    },
    {
      name: "declaration expression in decorator argument",
      source: "@dec(model { a: string }) model Foo {}",
    },
    { name: "templated declaration expression", source: "alias A = model M<T> { a: T };" },
  ];

  for (const testCase of cases) {
    it(testCase.name, () => compare(testCase.source));
  }

  describe("doc comments", () => {
    const docCases = [
      { name: "simple doc on model", source: "/** simple */\nmodel Foo {}" },
      {
        name: "multiline doc on model",
        source: "/**\n * first line\n * second line\n */\nmodel Foo {}",
      },
      {
        name: "param tag on operation",
        source: "/** @param x description */\nop read(x: string): string;",
      },
      {
        name: "returns tag on operation",
        source: "/** @returns description */\nop read(): string;",
      },
      { name: "return tag alias", source: "/** @return description */\nop read(): string;" },
      {
        name: "template tag",
        source: "/** @template T description */\nmodel Foo<T> { value: T; }",
      },
      { name: "prop tag", source: "/** @prop name description */\nmodel Foo { name: string; }" },
      { name: "errors tag", source: "/** @errors error description */\nop read(): string;" },
      { name: "unknown tag", source: "/** @custom description */\nmodel Foo {}" },
      {
        name: "multiple tags",
        source:
          "/**\n * Summary\n * @param x first\n * @param y - second\n * @returns value\n */\nop read(x: string, y: string): string;",
      },
      { name: "doc on namespace", source: "/** namespace doc */\nnamespace Foo { model Bar {} }" },
      { name: "doc on property", source: "model Foo { /** property doc */\nname: string; }" },
      { name: "doc after decorator", source: "@tag\n/** doc */\nmodel Foo {}" },
      { name: "multiple consecutive docs", source: "/** first */\n/** second */\nmodel Foo {}" },
    ];

    for (const testCase of docCases) {
      it(testCase.name, () => compareDocs(testCase.source));
    }
  });

  describe("comments", () => {
    const commentCases = [
      { name: "line comment", source: "// line\nmodel Foo {}" },
      { name: "block comment", source: "/* block */\nmodel Foo {}" },
      {
        name: "comments around property",
        source: "model Foo { /* before */ name: string; // after\n}",
      },
      { name: "multiple comments", source: "// one\n/* two */\nmodel Foo {} // three" },
      {
        name: "doc comment marked as docs",
        source: "/** doc */\nmodel Foo {}",
        options: { comments: true, docs: true },
      },
      {
        name: "doc parsing disabled leaves comment unmarked",
        source: "/** doc */\nmodel Foo {}",
        options: { comments: true, docs: false },
      },
    ];

    for (const testCase of commentCases) {
      it(testCase.name, () => compareComments(testCase.source, testCase.options));
    }
  });

  describe("triple-quoted string values", () => {
    const tripleCases = [
      { name: "strips opening and closing blank lines", source: 'alias T = """\n  hello\n  """;' },
      { name: "preserves relative indent", source: 'alias T = """\n  hello\n    world\n  """;' },
      { name: "normalizes crlf", source: 'alias T = """\r\n  hello\r\n  """;' },
      { name: "template head", source: 'alias T = """\n  hello ${name}\n  """;' },
      {
        name: "template escaped interpolation",
        source: 'alias T = """\n  hello \\${name}\n  """;',
      },
    ];

    for (const testCase of tripleCases) {
      it(testCase.name, () => {
        const tsResult = tsParse(testCase.source);
        const rustResult = rustParse(testCase.source) as RustAst;
        const tsLiteral =
          findFirstTs(tsResult, SyntaxKind.StringLiteral) ??
          findFirstTs(tsResult, SyntaxKind.StringTemplateHead);
        const rustLiteral =
          findFirstRust(rustResult, SyntaxKind.StringLiteral) ??
          findFirstRust(rustResult, SyntaxKind.StringTemplateHead);
        expect(rustLiteral?.value).toBe(tsLiteral.value);
      });
    }
  });

  describe("diagnostic parity", () => {
    const diagnosticCases = [
      { name: "missing brace after model", source: "model Foo" },
      { name: "missing model close brace", source: "model Foo { name: string;" },
      { name: "missing identifier after model", source: "model {}" },
      { name: "missing semicolon after import", source: 'import "./foo.tsp"' },
      { name: "reserved keyword identifier", source: "model Foo { private: string; }" },
      { name: "unexpected expression token", source: "alias Foo = ;" },
      { name: "unterminated string", source: 'alias Foo = "unterminated' },
      { name: "missing const equals", source: "const foo" },
      { name: "missing decorator declaration semicolon", source: "dec myDec(target: Type)" },
      { name: "invalid enum member value", source: "enum Foo { a: true }" },
      { name: "union missing body after extends", source: "union Foo extends string" },
      { name: "union missing open brace", source: "union Foo string {}" },
      { name: "decorators without declaration expression", source: "alias A = @foo string;" },
      {
        name: "unterminated declaration expression",
        source: "model Foo { x: model { a: string; }",
      },
    ];

    for (const testCase of diagnosticCases) {
      it(testCase.name, () => diagnostics(testCase.source));
    }
  });
});
