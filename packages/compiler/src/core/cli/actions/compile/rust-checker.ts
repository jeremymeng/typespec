// Experimental: `tsp compile --use-rust-checker` (or `TYPESPEC_USE_RUST_CHECKER=1`) checks the
// program with the Rust checker of `@typespec/compiler-rust` (docs/rust-port/plan.md, M13).
// That package is not a dependency of the compiler (it depends on the compiler); it is loaded
// dynamically, only when requested.
import { pathToFileURL } from "url";
import { resolveModule } from "../../../../module-resolver/module-resolver.js";
import { createResolveModuleHost } from "../../../module-host.js";
import type { CompilerOptions } from "../../../options.js";
import { getDirectoryPath, resolvePath } from "../../../path-utils.js";
import type { Program } from "../../../program.js";
import { compileWithProgramFactory } from "../../../program.js";
import type { CompilerHost } from "../../../types.js";
import type { CompileCliArgs } from "./args.js";

const bridgeSpecifier = "@typespec/compiler-rust/tsp-bridge";

interface RustCheckerBridge {
  createRustProgram(
    host: CompilerHost,
    mainFile: string,
    options: CompilerOptions,
  ): Promise<{ program: Program; shouldAbort: boolean }>;
}

export function isRustCheckerEnabled(
  args: Pick<CompileCliArgs, "use-rust-checker">,
  env: Record<string, string | undefined> = process.env,
): boolean {
  if (args["use-rust-checker"]) return true;
  const value = env.TYPESPEC_USE_RUST_CHECKER?.toLowerCase();
  return value === "1" || value === "true";
}

let bridgePromise: Promise<RustCheckerBridge> | undefined;

/** Return a `compile` equivalent that checks with the Rust checker. */
export async function loadRustCheckerCompile(
  host: CompilerHost,
  entrypoint: string,
): Promise<(host: CompilerHost, mainFile: string, options?: CompilerOptions) => Promise<Program>> {
  bridgePromise ??= loadBridge(host, entrypoint);
  const bridge = await bridgePromise;
  return (host, mainFile, options = {}) =>
    compileWithProgramFactory(host, options, () =>
      bridge.createRustProgram(host, mainFile, options),
    );
}

function listDirHierarchy(dir: string): string[] {
  const dirs = [];
  for (let current = dir; ;) {
    dirs.push(current);
    const parent = getDirectoryPath(current);
    if (parent === current) return dirs;
    current = parent;
  }
}

async function loadBridge(host: CompilerHost, entrypoint: string): Promise<RustCheckerBridge> {
  // eslint-disable-next-line no-console
  console.error("Experimental: checking with the Rust checker (@typespec/compiler-rust).\n");
  const explicit = process.env.TYPESPEC_COMPILER_RUST_PATH;
  const candidates: string[] = [];
  if (explicit) {
    candidates.push(resolvePath(process.cwd(), explicit, "dist/tsp-bridge.js"));
  }
  const baseDirs = [getDirectoryPath(entrypoint), process.cwd(), host.getExecutionRoot()];
  // Inside the package itself (its tests) and in the TypeSpec repo (sibling package).
  for (const dir of [
    ...baseDirs.flatMap(listDirHierarchy),
    resolvePath(host.getExecutionRoot(), "../compiler-rust"),
  ]) {
    try {
      const manifest = JSON.parse((await host.readFile(resolvePath(dir, "package.json"))).text);
      if (manifest.name === "@typespec/compiler-rust") {
        candidates.push(resolvePath(dir, "dist/tsp-bridge.js"));
      }
    } catch {}
  }
  for (const baseDir of baseDirs) {
    try {
      const resolved = await resolveModule(createResolveModuleHost(host), bridgeSpecifier, {
        baseDir,
        conditions: ["import"],
      });
      candidates.push(resolved.type === "file" ? resolved.path : resolved.mainFile);
    } catch {}
  }
  for (const candidate of candidates) {
    try {
      return (await import(pathToFileURL(candidate).href)) as RustCheckerBridge;
    } catch {}
  }
  try {
    const specifier = bridgeSpecifier;
    return (await import(specifier)) as RustCheckerBridge;
  } catch (error) {
    throw new Error(
      `--use-rust-checker requires @typespec/compiler-rust (built, with its native binding). ` +
        `Install it next to the project or set TYPESPEC_COMPILER_RUST_PATH to its package directory.\n${error}`,
      { cause: error },
    );
  }
}
