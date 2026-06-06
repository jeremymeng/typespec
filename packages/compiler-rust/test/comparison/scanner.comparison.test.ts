import { createRequire } from "node:module";
import { describe, expect, it } from "vitest";
import { createScanner, Token, TokenFlags } from "../../../compiler/src/core/scanner.js";

const require = createRequire(import.meta.url);
const native = require("../../index.js") as {
  scan(source: string): ScanResult;
};

interface TokenInfo {
  kind: number;
  pos: number;
  end: number;
  line: number;
  character: number;
  text: string;
  value?: string;
}

interface DiagnosticInfo {
  severity: string;
  code: string;
  message: string;
  pos: number;
  end: number;
}

interface ScanResult {
  tokens: TokenInfo[];
  diagnostics: DiagnosticInfo[];
}

function scanWithTs(source: string): ScanResult {
  const diagnostics: DiagnosticInfo[] = [];
  const scanner = createScanner(source, (diagnostic) => {
    diagnostics.push({
      severity: diagnostic.severity,
      code: diagnostic.code,
      message: diagnostic.message,
      pos: diagnostic.target.pos,
      end: diagnostic.target.end,
    });
  });
  const tokens: TokenInfo[] = [];
  const templates: { flags: TokenFlags; braceDepth: number }[] = [];

  while (true) {
    let token = scanner.scan();
    const template = templates.at(-1);
    if (template) {
      if (token === Token.OpenBrace) {
        template.braceDepth++;
      } else if (token === Token.CloseBrace) {
        if (template.braceDepth === 0) {
          token = scanner.reScanStringTemplate(template.flags);
        } else {
          template.braceDepth--;
        }
      }
    }

    const position = scanner.file.getLineAndCharacterOfPosition(scanner.tokenPosition);
    tokens.push({
      kind: scanner.token,
      pos: scanner.tokenPosition,
      end: scanner.position,
      line: position.line,
      character: position.character,
      text: scanner.getTokenText(),
      value: tokenValue(scanner.token, scanner.getTokenValue()),
    });

    switch (token) {
      case Token.StringTemplateHead:
        templates.push({ flags: scanner.tokenFlags, braceDepth: 0 });
        break;
      case Token.StringTemplateMiddle:
        templates[templates.length - 1] = { flags: scanner.tokenFlags, braceDepth: 0 };
        break;
      case Token.StringTemplateTail:
        templates.pop();
        break;
      case Token.EndOfFile:
        return { tokens, diagnostics };
    }
  }
}

function tokenValue(token: Token, value: string): string | undefined {
  switch (token) {
    case Token.Identifier:
    case Token.NumericLiteral:
    case Token.StringLiteral:
    case Token.StringTemplateHead:
    case Token.StringTemplateMiddle:
    case Token.StringTemplateTail:
    case Token.DocText:
      return value;
    default:
      return undefined;
  }
}

function compare(source: string) {
  const tsResult = scanWithTs(source);
  const rustResult = native.scan(source);
  expect(rustResult.diagnostics).toEqual(tsResult.diagnostics);
  expect(rustResult.tokens).toEqual(tsResult.tokens);
}

describe("scanner comparison: Rust vs TS", () => {
  const cases = [
    { name: "empty input", source: "" },
    { name: "bom only", source: "\u{feff}" },
    { name: "single identifier", source: "Foo" },
    { name: "unicode identifier", source: "ñ ñ" },
    { name: "other unicode identifier starts", source: "\u{1885} \u{2118} \u{309b}" },
    { name: "backticked identifiers", source: "`a` `01-01` `x\\`x` `a\\n\\t\\`b`" },
    { name: "backticked non ascii normalization", source: "`😍` `ñ is ñ` ñ" },
    { name: "model declaration", source: "model Foo { x: string }" },
    { name: "decorator expression", source: '@foo(1,"hello",foo)' },
    { name: "directive expression", source: '#suppress foo "hello")' },
    { name: "intersection", source: "A&B" },
    { name: "projection punctuation", source: "<= >= && || == projection if => != @@" },
    { name: "hash punctuation", source: "#{ #[ # @ @@ :: ... ; , ? :" },
    { name: "all braces", source: "{ } ( ) [ ]" },
    { name: "operators", source: "+ - * / ! = < > & |" },
    { name: "keywords", source: "import model scalar namespace using op enum alias is interface union" },
    { name: "other keywords", source: "extends fn true false return void never unknown valueof typeof" },
    { name: "modifier keywords", source: "extern internal const init" },
    { name: "reserved keywords", source: "statemachine macro package metadata env arg declare array struct" },
    { name: "more reserved keywords", source: "record module mod sym context prop property scenario pub sub typeref trait" },
    { name: "final reserved keywords", source: "this self super keyof with implements impl satisfies flag auto partial private public protected sealed local async" },
    { name: "keyword collision", source: "outerface famespace notanamespace notaninterface" },
    { name: "numeric literals", source: "42 0xBEEF 0b1010 1.5e4 314.0e-2 1e+1000" },
    { name: "signed numeric literals", source: "+1 -2 +3.5e-1 -4e+2" },
    { name: "bigint-like splits like TS", source: "1n 0x1n" },
    { name: "hex digit diagnostic", source: "0x " },
    { name: "binary digit diagnostic", source: "0b " },
    { name: "decimal digit diagnostic", source: "1. 1e 1e+" },
    { name: "empty string", source: '""' },
    { name: "escaped string", source: '"Hello world \\r\\n \\t \\" \\\\ !"' },
    { name: "invalid escape", source: '"Hello world \\d"' },
    { name: "unterminated string", source: '"More\nthan' },
    { name: "triple quoted string", source: '"""\n  one\n  two\n  """' },
    { name: "triple quoted crlf", source: '"""\r\none\r\ntwo\r\n"""' },
    { name: "triple quote start diagnostic", source: '"""no newline\n"""' },
    { name: "triple quote end diagnostic", source: '"""\nno newline"""' },
    { name: "string template simple", source: '"hello ${name}"' },
    { name: "string template middle", source: '"a ${b} c ${d} e"' },
    { name: "string template nested braces", source: '"a ${{x: {y: 1}}} b"' },
    { name: "triple string template", source: '"""\n  a ${b} c\n  """' },
    { name: "single line comment", source: "// hello\nmodel Foo" },
    { name: "single line comment at eof", source: "// hello" },
    { name: "multi line comment", source: "/* foo\n*bar\n*/" },
    { name: "doc comment", source: "/** @tag hello */\nmodel Foo {}" },
    { name: "unterminated comment", source: "/* unterminated" },
    { name: "position tracking", source: "a x\raa x\r\naaa x\naaaa x\u{2028}aaaaa x\u{2029}aaaaaa x" },
    { name: "non ascii whitespace", source: "a\u{2028}b\u{2029}c\u{200e}d\u{200f}e" },
    { name: "invalid ascii", source: "~ % ^ '" },
    { name: "invalid unicode", source: "😍" },
    { name: "merge conflict head", source: "<<<<<<< HEAD\nmodel A {}\n=======\nmodel B {}\n>>>>>>> branch" },
  ];

  for (const testCase of cases) {
    it(testCase.name, () => compare(testCase.source));
  }

  it("fuzzes printable inputs without panics or token-count drift", () => {
    let seed = 0x5eed;
    const chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789{}()[]<>:=,.;@#&|!?+-*/\"` \t\r\nñΩ😍";
    const next = () => {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      return seed;
    };

    for (let i = 0; i < 100; i++) {
      const length = next() % 200;
      let source = "";
      for (let j = 0; j < length; j++) {
        source += chars[next() % chars.length];
      }
      const tsResult = scanWithTs(source);
      const rustResult = native.scan(source);
      expect(rustResult.tokens.length, source).toBe(tsResult.tokens.length);
    }
  });
});
