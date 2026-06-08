import { readFileSync, writeFileSync } from "node:fs";

const decoratorJsExport = `export { createDecoratorRegistry, createIntrinsicDecoratorRegistry, intrinsicDecorators, registerDecorator } from './intrinsic-decorators.js'\n`;
const numericJsExport = `
export function numericFromSerialized(value) {
  if (value?.literalNumericBigint !== undefined && value.literalNumericBigint !== null) {
    return { kind: "integer", bigint: value.literalNumericBigint };
  }
  if (value?.literalNumericString !== undefined && value.literalNumericString !== null) {
    return { kind: "decimal", string: value.literalNumericString };
  }
  if (value?.literalNumericNumber !== undefined && value.literalNumericNumber !== null) {
    return { kind: "number", number: value.literalNumericNumber };
  }
  return null;
}

export function numericEquals(rustValue, tsNumeric) {
  if (rustValue === null || rustValue === undefined) {
    return false;
  }
  const expected = tsNumeric.toString();
  switch (rustValue.kind) {
    case "integer":
      return rustValue.bigint.toString() === expected;
    case "decimal":
      return rustValue.string === expected;
    case "number":
      return String(rustValue.number) === expected;
    default:
      return false;
  }
}
`;
const dtsExport = `
export type DecoratorContext = {
  getProperty(target: unknown, key: string): unknown;
  setProperty(target: unknown, key: string, value: unknown): void;
  getName(target: unknown): string | undefined;
  getKind(target: unknown): string;
  reportDiagnostic(diagnostic: { code?: string; message?: string; target?: unknown }): void;
};

export type DecoratorImplementation = (
  context: DecoratorContext,
  target: unknown,
  ...args: unknown[]
) => void;

export declare const intrinsicDecorators: Record<string, DecoratorImplementation>;
export declare function registerDecorator(
  registry: Record<string, DecoratorImplementation>,
  name: string,
  fn: DecoratorImplementation,
): Record<string, DecoratorImplementation>;
export declare function createDecoratorRegistry(
  decorators?: Record<string, DecoratorImplementation>,
): Record<string, DecoratorImplementation>;
export declare function createIntrinsicDecoratorRegistry(): Record<string, DecoratorImplementation>;
`;
const numericDtsExport = `

export type SerializedNumeric =
  | { kind: "integer"; bigint: bigint }
  | { kind: "decimal"; string: string }
  | { kind: "number"; number: number };

export declare function numericFromSerialized(value: {
  literalNumericBigint?: bigint;
  literalNumericString?: string;
  literalNumericNumber?: number;
}): SerializedNumeric | null;
export declare function numericEquals(
  rustValue: SerializedNumeric | null,
  tsNumeric: { toString(): string },
): boolean;
`;
const registerDts = `export declare function registerDecorator(
  registry: Record<string, DecoratorImplementation>,
  name: string,
  fn: DecoratorImplementation,
): Record<string, DecoratorImplementation>;
`;

function appendIfMissing(path, marker, text, replacePattern) {
  let content = readFileSync(path, "utf8");
  if (!content.includes(marker)) {
    if (replacePattern && replacePattern.test(content)) {
      content = content.replace(replacePattern, text.trimEnd());
      writeFileSync(path, content, "utf8");
      return;
    }
    writeFileSync(path, `${content.trimEnd()}\n${text}`, "utf8");
  }
}

appendIfMissing(
  "index.js",
  "registerDecorator",
  decoratorJsExport,
  /export \{ createDecoratorRegistry, createIntrinsicDecoratorRegistry, intrinsicDecorators \} from '\.\/intrinsic-decorators\.js'\n?/,
);
appendIfMissing("index.js", "numericFromSerialized", numericJsExport);
let dts = readFileSync("index.d.ts", "utf8");
if (!dts.includes("DecoratorImplementation")) {
  writeFileSync("index.d.ts", `${dts.trimEnd()}\n${dtsExport}`, "utf8");
} else if (!dts.includes("registerDecorator")) {
  dts = dts.replace(
    "export declare function createDecoratorRegistry",
    `${registerDts}export declare function createDecoratorRegistry`,
  );
  writeFileSync("index.d.ts", dts, "utf8");
}
appendIfMissing("index.d.ts", "numericFromSerialized", numericDtsExport);
