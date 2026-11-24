// M12: run real emitters (`$onEmit`) over the Program adapter, like TS `compile`'s emitter
// stage: emitters are resolved by package name, their options validated against the
// library's `$lib.emitter.options` schema, and `$onEmit` called with a TS-shaped `EmitContext`
// whose `program.host` writes through an `EmitterHost` (`host-fs.ts`).

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { Diagnostic } from "./diagnostics.js";
import { getLibraryRoot } from "./host.js";
import { joinPaths, resolvePath } from "./path-utils.js";
import type { Program } from "./program.js";

/** TS `CompilerOptions` subset that drives the emitter stage. */
export interface EmitCompilerOptions {
  /** Emitter package names, like TS `emit`. */
  emit?: string[];
  /** Options by emitter name; `emitter-output-dir` overrides the output directory. */
  options?: Record<string, Record<string, unknown>>;
  /** Default output root (TS `outputDir`); emitters write to `<outputDir>/<name>`. */
  outputDir?: string;
  noEmit?: boolean;
  dryRun?: boolean;
  [key: string]: unknown;
}

interface LoadedEmitter {
  name: string;
  emitFunction: (context: unknown) => unknown;
  emitterOutputDir: string;
  options: Record<string, unknown>;
  capabilities?: { dryRun?: boolean };
}

/**
 * TS `loadEmitters`: resolve each emitter, validate its options and report
 * `emitter-not-found` / `invalid-emitter` / option schema diagnostics on the program.
 */
export async function loadEmitters(
  program: Program,
  compilerOptions: EmitCompilerOptions,
  jsModules?: Record<string, Record<string, unknown>>,
): Promise<LoadedEmitter[]> {
  const names = compilerOptions.noEmit ? [] : (compilerOptions.emit ?? []);
  const emitters: LoadedEmitter[] = [];
  for (const name of names) {
    const exports = jsModules?.[name] ?? (await importEmitter(name));
    if (exports === undefined) {
      program.reportDiagnostic(
        createCompilerDiagnostic(
          "emitter-not-found",
          `Cannot find emitter with name ${name} in ${compilerOptions.projectRoot ?? "/"}`,
        ),
      );
      continue;
    }
    const lib = exports.$lib as
      | {
          name?: string;
          emitter?: { options?: unknown };
          emitterOptionValidator?: { validate(value: unknown, target: unknown): Diagnostic[] };
          capabilities?: { dryRun?: boolean };
        }
      | undefined;
    const metadataName = lib?.name ?? name;
    const { "emitter-output-dir": outputDir, ...options } =
      compilerOptions.options?.[metadataName] ?? {};
    const emitterOutputDir =
      (outputDir as string | undefined) ??
      [compilerOptions.outputDir, metadataName].filter((x) => x !== undefined).join("/");
    const emitFunction = exports.$onEmit;
    if (typeof emitFunction !== "function") {
      program.reportDiagnostic(
        createCompilerDiagnostic(
          "invalid-emitter",
          `Requested emitter package ${name} does not provide an "$onEmit" function.`,
        ),
      );
      continue;
    }
    if (lib?.emitter?.options) {
      const diagnostics = lib.emitterOptionValidator?.validate(options, NoTarget);
      if (diagnostics && diagnostics.length > 0) {
        program.reportDiagnostics(diagnostics);
        continue;
      }
    }
    emitters.push({
      name: metadataName,
      emitFunction: emitFunction as LoadedEmitter["emitFunction"],
      emitterOutputDir,
      options,
      capabilities: lib?.capabilities,
    });
  }
  return emitters;
}

/** TS emitter stage: call each emitter's `$onEmit` (skipping non-dry-run emitters on dry runs). */
export async function runEmitters(
  program: Program,
  emitters: readonly LoadedEmitter[],
  compilerOptions: EmitCompilerOptions,
): Promise<void> {
  for (const emitter of emitters) {
    if (compilerOptions.dryRun && !emitter.capabilities?.dryRun) continue;
    await emitter.emitFunction({
      program,
      emitterOutputDir: emitter.emitterOutputDir,
      options: emitter.options,
      perf: createPerfReporter(),
    });
  }
}

const NoTarget = Symbol.for("NoTarget");

function createCompilerDiagnostic(code: string, message: string): Diagnostic {
  return { code, severity: "error", message, target: NoTarget } as unknown as Diagnostic;
}

function createPerfReporter() {
  const measures: Record<string, number> = {};
  return {
    measures,
    startTimer(label: string) {
      const start = performance.now();
      return { end: () => (measures[label] = performance.now() - start) };
    },
    time<T>(label: string, fn: () => T): T {
      const start = performance.now();
      try {
        return fn();
      } finally {
        measures[label] = performance.now() - start;
      }
    },
    async timeAsync<T>(label: string, fn: () => Promise<T>): Promise<T> {
      const start = performance.now();
      try {
        return await fn();
      } finally {
        measures[label] = performance.now() - start;
      }
    },
    report(label: string, duration: number) {
      measures[label] = duration;
    },
  };
}

/** Import an emitter package's JS entrypoint (`exports["."]` / `main`), resolved like libraries. */
async function importEmitter(name: string): Promise<Record<string, unknown> | undefined> {
  let root: string;
  try {
    root = getLibraryRoot(name);
  } catch {
    return undefined;
  }
  const manifest = JSON.parse(readFileSync(joinPaths(root, "package.json"), "utf8"));
  const entry = resolvePackageEntry(manifest);
  if (entry === undefined) return undefined;
  const path = resolvePath(joinPaths(root, entry));
  return (await import(pathToFileURL(path).href)) as Record<string, unknown>;
}

function resolvePackageEntry(manifest: { exports?: unknown; main?: string }): string | undefined {
  const pick = (target: unknown): string | undefined => {
    if (typeof target === "string") return target;
    if (target && typeof target === "object") {
      const conditions = target as Record<string, unknown>;
      for (const key of ["import", "default", "node"]) {
        const found = pick(conditions[key]);
        if (found) return found;
      }
    }
    return undefined;
  };
  const exports = manifest.exports;
  if (exports !== undefined) {
    const main =
      typeof exports === "string" || (exports && !("." in (exports as object)))
        ? exports
        : (exports as Record<string, unknown>)["."];
    const found = pick(main);
    if (found) return found;
  }
  return manifest.main ?? "index.js";
}
