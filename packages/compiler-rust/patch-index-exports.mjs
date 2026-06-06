import { readFileSync, writeFileSync } from "node:fs";

const jsExport = `export { createDecoratorRegistry, createIntrinsicDecoratorRegistry, intrinsicDecorators, registerDecorator } from './intrinsic-decorators.js'\n`;
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
  jsExport,
  /export \{ createDecoratorRegistry, createIntrinsicDecoratorRegistry, intrinsicDecorators \} from '\.\/intrinsic-decorators\.js'\n?/,
);
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
