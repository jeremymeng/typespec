// M13: `tsp compile --use-rust-checker`. The TS CLI (`packages/compiler`,
// `core/cli/actions/compile/rust-checker.ts`) dynamically imports this module and runs its
// `compile` pipeline (progress reporting, emitter stage) over the program created here:
// sources are loaded and checked by the Rust checker, then the JS stages of TS `createProgram`
// run over the Program adapter — emitter loading, `#suppress` handling, linter ruleset
// resolution, decorators, validators and the linter — using the running compiler's own
// modules (loaded from its `dist`, so state keys and `instanceof` checks are shared).
//
// Diagnostics are converted to TS `SourceLocation` targets (real `SourceFile`s), so the CLI
// formats them with line/column exactly like the TS checker.

import { SyntaxKind } from "@typespec/compiler/ast";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type { CheckProgramResultJs, ProgramDiagnosticInfo, SuppressDirectiveJs } from "../index.js";
import {
  createImplementationRegistry,
  registerLibraryDecorators,
  runCompilerValidators,
  runDecorators,
} from "./decorator-registration.js";
import {
  checkProgram,
  createNodeProgramHost,
  setCompilerRoot,
  type ProgramHost,
  type ProjectConfigInfo,
} from "./host.js";
import { getDirectoryPath, joinPaths, normalizePath } from "./path-utils.js";
import { createProgram, type NodeShim } from "./program.js";

/** Subset of the TS `CompilerHost` the bridge uses. */
export interface BridgeCompilerHost {
  getExecutionRoot(): string;
  getJsImport(path: string): Promise<Record<string, any>>;
  readFile(path: string): Promise<{ text: string; path: string }>;
  stat(path: string): Promise<{ isFile(): boolean; isDirectory(): boolean }>;
  realpath(path: string): Promise<string>;
  [key: string]: any;
}

/** Result shape of TS `createProgram`. */
export interface BridgeProgramResult {
  program: any;
  shouldAbort: boolean;
}

const NoTarget = Symbol.for("NoTarget");

/** Internal modules of the running compiler (not part of its public API). */
interface CompilerInternals {
  linter: Record<string, any>;
  diagnosticCode: Record<string, any>;
  messages: Record<string, any>;
  moduleResolver: Record<string, any>;
  moduleHost: Record<string, any>;
  sourceLoader: Record<string, any>;
  sourceFile: Record<string, any>;
  configLoader: Record<string, any>;
  yaml: Record<string, any>;
}

async function loadCompilerInternals(compilerRoot: string): Promise<CompilerInternals> {
  const load = (path: string) =>
    import(pathToFileURL(joinPaths(compilerRoot, "dist/src", path)).href) as Promise<
      Record<string, any>
    >;
  const [
    linter,
    diagnosticCode,
    messages,
    moduleResolver,
    moduleHost,
    sourceLoader,
    sourceFile,
    configLoader,
    yaml,
  ] = await Promise.all([
    load("core/linter.js"),
    load("core/diagnostic-code.js"),
    load("core/messages.js"),
    load("module-resolver/module-resolver.js"),
    load("core/module-host.js"),
    load("core/source-loader.js"),
    load("core/source-file.js"),
    load("config/config-loader.js"),
    load("yaml/parser.js"),
  ]);
  return {
    linter,
    diagnosticCode,
    messages,
    moduleResolver,
    moduleHost,
    sourceLoader,
    sourceFile,
    configLoader,
    yaml,
  };
}

/**
 * Create a TS-compatible program for `mainFile` with the Rust checker: the drop-in
 * replacement for TS `createProgram` used by `tsp compile --use-rust-checker`.
 */
export async function createRustProgram(
  host: BridgeCompilerHost,
  mainFile: string,
  options: Record<string, any> = {},
): Promise<BridgeProgramResult> {
  const compilerRoot = normalizePath(host.getExecutionRoot());
  setCompilerRoot(compilerRoot);
  const internals = await loadCompilerInternals(compilerRoot);
  const { createDiagnostic } = internals.messages;
  const timer = startTimer();

  // --- Load and check with the Rust checker --------------------------------------------
  const texts = new Map<string, string>();
  const libraryRoots = new Set<string>();
  const baseHost = createNodeProgramHost({
    loadProjectConfig: (dir) => readProjectConfig(internals, dir),
  });
  const programHost: ProgramHost = {
    readFile(path) {
      const text = baseHost.readFile(path);
      if (text !== undefined) texts.set(normalizePath(path), text);
      return text;
    },
    resolveModule(specifier, baseDir) {
      const resolution = baseHost.resolveModule(specifier, baseDir);
      if (resolution.library && resolution.resolvedPath) {
        const root = findPackageRoot(resolution.resolvedPath);
        if (root) libraryRoots.add(root);
      }
      return resolution;
    },
    resolveEntrypoint: (path) => baseHost.resolveEntrypoint(path),
  };
  const stdlib = !options.nostdlib;
  const features: string[] | undefined = options.configFile?.features;
  const check = (libraryFeatures?: { root: string; features: string[] }[]) =>
    checkProgram(mainFile, programHost, undefined, {
      collectDecoratorApplications: true,
      stdlib,
      features,
      libraryFeatures,
    });
  let result: CheckProgramResultJs = check();
  // Libraries enable features for their own code in their own `tspconfig.yaml` (TS
  // `resolveLibraryFeatures`); they are only known once the libraries are resolved.
  const libraryFeatures = await resolveLibraryFeatures(host, internals, libraryRoots);
  if (libraryFeatures.length > 0) result = check(libraryFeatures);
  const loaderTime = timer();

  const getSourceFile = createSourceFileCache(internals, texts);
  const fileContexts = new Map(
    result.sourceFiles.map((file) => [file.path, file.context ?? undefined]),
  );

  // --- Program adapter, completed with the TS `Program` members the pipeline uses -------
  const program: any = createProgram(result);
  const projectRoot = getDirectoryPath(options.config ?? mainFile);
  const emitters: any[] = [];
  const requireImports = new Map<string, string>();
  const loadedLibraries = new Map<string, { alias?: string }>();
  const aliases = new Map<string, string>();
  const diagnostics: any[] = [];
  let error = false;
  let continueToNextStage = true;
  let currentStage = "checking";
  const tracer = { trace() {}, sub: () => tracer };
  const runtimeStats: Record<string, any> = { loader: loaderTime, resolver: 0, checker: 0 };

  for (const map of [program.sourceFiles, program.jsSourceFiles]) {
    for (const [path, script] of map as Map<string, any>) {
      script.file = getSourceFile(path);
    }
  }
  attachScriptParents(program._nodes, getSourceFile);

  const registry = createImplementationRegistry();
  for (const file of result.sourceFiles) {
    if (file.kind !== "js") continue;
    const exports = await host.getJsImport(file.path);
    registerLibraryDecorators(registry, exports);
    const lib = exports.$lib as { name?: string; alias?: string } | undefined;
    if (lib?.name && lib.alias) aliases.set(lib.name, lib.alias);
  }
  // TS `sourceResolution.loadedLibraries`: packages TypeSpec files were imported from.
  for (const root of libraryRoots) {
    const name = readPackageName(root);
    if (name !== undefined) loadedLibraries.set(name, { alias: aliases.get(name) });
  }

  Object.assign(program, {
    compilerOptions: { ...options },
    host,
    projectRoot,
    emitters,
    diagnostics,
    tracer,
    trace: () => {},
    stats: {
      complexity: { createdTypes: result.types.length, finishedTypes: result.types.length },
      runtime: runtimeStats,
    },
    setCurrentStage(stage: string) {
      currentStage = stage;
    },
    hasError: () => error,
    reportDiagnostic,
    reportDiagnostics: (list: readonly any[]) => list.forEach(reportDiagnostic),
    getOption: (key: string) => options.miscOptions?.[key],
    getSourceFileLocationContext,
    onValidate(callback: (program: any) => unknown) {
      registry.validators.push(callback);
    },
  });
  Object.defineProperty(program, "currentStage", { get: () => currentStage });

  // --- Emitters (TS loads them before checking) -----------------------------------------
  const basedir = getDirectoryPath(mainFile) || "/";
  for (const name of options.noEmit ? [] : (options.emit ?? [])) {
    const emitter = await loadEmitter(name);
    if (emitter) emitters.push(emitter);
  }

  // --- Suppression and linter configuration ---------------------------------------------

  const codeResolver = internals.diagnosticCode.createDiagnosticCodeResolver([
    ...[...loadedLibraries].map(([name, { alias }]) => ({ name, alias })),
    ...(loadedLibraries.has(internals.linter.builtInLinterLibraryName)
      ? []
      : [{ name: internals.linter.builtInLinterLibraryName }]),
  ]);
  program.diagnosticCodeResolver = codeResolver;
  const directives = result.suppressDirectives;
  reportDiagnostics(
    findDuplicateSuppressions(directives).map((directive) =>
      createDiagnostic({
        code: "duplicate-suppression",
        format: { code: directive.code },
        target: directiveLocation(directive),
      }),
    ),
  );
  for (const directive of directives) {
    if (!directive.project) continue;
    const conflict = codeResolver.getAmbiguousShortName(directive.code);
    if (conflict) {
      reportDiagnostic(
        createDiagnostic({
          code: "ambiguous-short-name",
          format: {
            shortName: conflict.shortName,
            candidates: internals.diagnosticCode.formatShortNameCandidates(conflict.candidates),
          },
          target: directiveLocation(directive),
        }),
      );
    }
  }

  const enabledBuiltInRules: string[] = [];
  const linter = internals.linter.createLinter(
    program,
    (name: string) => loadLibrary(basedir, name),
    codeResolver,
  );
  linter.registerLinterLibrary(
    internals.linter.builtInLinterLibraryName,
    wrapBuiltInLinterLibrary(internals.linter.createBuiltInLinterLibrary(), enabledBuiltInRules),
  );
  await configureLinter(linter, codeResolver);

  // --- Checking: Rust diagnostics, then the JS decorators --------------------------------
  const rustLintDiagnostics: ProgramDiagnosticInfo[] = [];
  for (const diagnostic of result.diagnostics) {
    if (diagnostic.code.includes("/")) {
      rustLintDiagnostics.push(diagnostic);
    } else {
      reportDiagnostic(fromRustDiagnostic(diagnostic));
    }
  }
  const checkerTimer = startTimer();
  runDecorators(program, result, registry, { reportMissingImplementations: true });
  runtimeStats.checker = checkerTimer();
  if (!continueToNextStage) return { program, shouldAbort: true };

  // --- Validation --------------------------------------------------------------------------
  currentStage = "validating";
  const validationTimer = startTimer();
  runtimeStats.validation = { total: 0, validators: {} };
  if (stdlib) await runCompilerValidators(program);
  for (const validator of registry.validators) {
    const result = await validator(program);
    if (Array.isArray(result)) program.reportDiagnostics(result);
  }
  runtimeStats.validation.total = validationTimer();
  for (const [requiredImport, emitterName] of requireImports) {
    if (!loadedLibraries.has(requiredImport)) {
      reportDiagnostic(
        createDiagnostic({
          code: "missing-import",
          format: { requiredImport, emitterName },
          target: NoTarget,
        }),
      );
    }
  }
  if (!continueToNextStage) return { program, shouldAbort: true };

  // --- Linting: the built-in rules ran in Rust; report them when the ruleset enables them --
  currentStage = "linting";
  const lintResult = await linter.lint();
  runtimeStats.linter = lintResult.stats.runtime;
  const ruleOrder = (code: string) => enabledBuiltInRules.indexOf(code);
  rustLintDiagnostics
    .filter((diagnostic) => ruleOrder(diagnostic.code) !== -1)
    .map((diagnostic, index) => ({ diagnostic, index }))
    .sort(
      (a, b) => ruleOrder(a.diagnostic.code) - ruleOrder(b.diagnostic.code) || a.index - b.index,
    )
    .forEach(({ diagnostic }) => reportDiagnostic(fromRustDiagnostic(diagnostic)));
  program.reportDiagnostics(lintResult.diagnostics);
  return { program, shouldAbort: false };

  // --- Helpers -------------------------------------------------------------------------------

  function reportDiagnostics(list: readonly any[]) {
    list.forEach(reportDiagnostic);
  }

  /** TS `reportDiagnostic` + `shouldSuppress`, over converted targets. */
  function reportDiagnostic(diagnostic: any): void {
    const located = toSourceLocation(diagnostic.target);
    if (diagnostic.code !== "error" && located?.suppressible) {
      const directive = findSuppressingDirective(diagnostic.code, located.raw);
      if (directive) {
        if (diagnostic.severity !== "error") return;
        diagnostics.push({
          severity: "error",
          code: "suppress-error",
          message: "Errors cannot be suppressed.",
          target: directiveLocation(directive),
        });
      }
    }
    diagnostic = located ? { ...diagnostic, target: located.location } : diagnostic;
    if (diagnostic.severity === "error") continueToNextStage = false;
    if (options.warningAsError && diagnostic.severity === "warning") {
      diagnostic = { ...diagnostic, severity: "error" };
    }
    if (diagnostic.severity === "error") error = true;
    diagnostics.push(diagnostic);
  }

  /**
   * TS `findDirectiveSuppressingOnNode`: the innermost node (owner span) around the target with
   * a `#suppress` directive for the code; its first matching directive.
   */
  function findSuppressingDirective(
    code: string,
    raw: { path: string; pos: number; end: number },
  ): SuppressDirectiveJs | undefined {
    const resolved = codeResolver.resolveCode(code);
    let best: SuppressDirectiveJs | undefined;
    for (const directive of directives) {
      if (directive.file !== raw.path) continue;
      if (directive.ownerPos > raw.pos || directive.ownerEnd < raw.end) continue;
      if (codeResolver.resolveCode(directive.code) !== resolved) continue;
      const size = directive.ownerEnd - directive.ownerPos;
      if (!best || size < best.ownerEnd - best.ownerPos) best = directive;
    }
    return best;
  }

  function directiveLocation(directive: SuppressDirectiveJs) {
    return { file: getSourceFile(directive.file), pos: directive.pos, end: directive.end };
  }

  function fromRustDiagnostic(diagnostic: ProgramDiagnosticInfo) {
    return {
      code: diagnostic.code,
      severity: diagnostic.severity,
      message: diagnostic.message,
      target: diagnostic.file
        ? { kind: "rust-diagnostic", ...diagnostic, file: diagnostic.file }
        : NoTarget,
    };
  }

  /**
   * Resolve a diagnostic target to a TS `SourceLocation` (with the identifier span, like
   * `getSourceLocation(target, { locateId: true })`) and the raw span used for suppression.
   */
  function toSourceLocation(target: any):
    | {
        location: any;
        raw: { path: string; pos: number; end: number };
        suppressible: boolean;
      }
    | undefined {
    if (target === undefined || target === null || typeof target !== "object") return undefined;
    if (target.kind === "path-target") return undefined;
    let node: any = target;
    if (target.kind === "rust-diagnostic") {
      node = target;
    } else if ("entityKind" in target || "valueKind" in target) {
      node = target.node;
    } else if (!("nodeId" in target)) {
      // TS locations (`{ file: SourceFile, pos, end }`) and other targets pass through.
      return undefined;
    }
    if (!node || typeof node.file !== "string") return undefined;
    const path = node.file as string;
    const raw = { path, pos: node.pos as number, end: node.end as number };
    const hasId = node.idPos !== undefined && node.idPos !== null;
    return {
      location: {
        file: getSourceFile(path),
        pos: hasId ? node.idPos : node.pos,
        end: hasId ? (node.idEnd ?? node.idPos) : node.end,
      },
      raw,
      suppressible: true,
    };
  }

  function getSourceFileLocationContext(fileOrPath: string | { path: string }) {
    const path = typeof fileOrPath === "string" ? fileOrPath : fileOrPath.path;
    const context = fileContexts.get(path);
    if (context === "compiler") return { type: "compiler" };
    if (context === "library") {
      return { type: "library", metadata: { type: "module", name: undefined } };
    }
    return { type: "project" };
  }

  function findDuplicateSuppressions(list: readonly SuppressDirectiveJs[]) {
    const seen = new Set<string>();
    const duplicates: SuppressDirectiveJs[] = [];
    for (const directive of list) {
      if (!directive.project) continue;
      const key = `${directive.file}\0${directive.ownerPos}\0${directive.ownerEnd}\0${directive.code}`;
      if (seen.has(key)) duplicates.push(directive);
      else seen.add(key);
    }
    return duplicates;
  }

  async function resolveLibraryModule(
    specifier: string,
    baseDir: string,
  ): Promise<[any, readonly any[]]> {
    try {
      return [
        await internals.moduleResolver.resolveModule(
          internals.moduleHost.createResolveModuleHost(host),
          specifier,
          { baseDir, conditions: ["import"] },
        ),
        [],
      ];
    } catch (e: any) {
      if (e?.name === "ResolveModuleError" || typeof e?.code === "string") {
        return [
          undefined,
          [internals.sourceLoader.moduleResolutionErrorToDiagnostic(e, specifier, NoTarget)],
        ];
      }
      throw e;
    }
  }

  /** TS `loadLibrary`. */
  async function loadLibrary(dir: string, name: string, isEmitter = false) {
    const [module, resolveDiagnostics] = await resolveLibraryModule(name, dir);
    if (!module) {
      if (isEmitter && resolveDiagnostics.some((d) => d.code === "import-not-found")) {
        reportDiagnostic(
          createDiagnostic({
            code: "emitter-not-found",
            format: { emitterPackage: name },
            target: NoTarget,
          }),
        );
      } else {
        reportDiagnostics(resolveDiagnostics);
      }
      return undefined;
    }
    const entrypointPath = module.type === "file" ? module.path : module.mainFile;
    const esmExports = await host.getJsImport(entrypointPath);
    const definition = esmExports.$lib;
    const metadata =
      module.type === "file"
        ? { type: "file", name: definition?.name }
        : {
            type: "module",
            name: module.manifest.name,
            ...(module.manifest.homepage ? { homepage: module.manifest.homepage } : {}),
            ...(module.manifest.bugs?.url ? { bugs: { url: module.manifest.bugs.url } } : {}),
            ...(module.manifest.version ? { version: module.manifest.version } : {}),
          };
    const linterDef = esmExports.$linter;
    return {
      module,
      entrypoint: { file: { path: entrypointPath }, esmExports },
      metadata,
      definition,
      linter: linterDef && internals.linter.resolveLinterDefinition(name, linterDef),
    };
  }

  /** TS `loadEmitter`. */
  async function loadEmitter(name: string) {
    const library = await loadLibrary(basedir, name, true);
    if (library === undefined) return undefined;
    const { entrypoint, metadata, definition } = library;
    const emitFunction = entrypoint.esmExports.$onEmit;
    const emittersOptions = options.options ?? {};
    let { "emitter-output-dir": emitterOutputDir, ...emitterOptions } =
      emittersOptions[metadata.name ?? name] ?? {};
    if (emitterOutputDir === undefined) {
      emitterOutputDir = [options.outputDir, metadata.name]
        .filter((x) => x !== undefined)
        .join("/");
    }
    for (const lib of definition?.requireImports ?? []) {
      requireImports.set(lib, definition.name);
    }
    if (emitFunction === undefined) {
      reportDiagnostic(
        createDiagnostic({
          code: "invalid-emitter",
          format: { emitterPackage: name },
          target: NoTarget,
        }),
      );
      return undefined;
    }
    if (definition?.emitter?.options) {
      const optionDiagnostics = definition.emitterOptionValidator?.validate(
        emitterOptions,
        options.configFile?.file
          ? { kind: "path-target", path: ["options", name], script: options.configFile.file }
          : NoTarget,
      );
      if (optionDiagnostics && optionDiagnostics.length > 0) {
        reportDiagnostics(optionDiagnostics);
        return undefined;
      }
    }
    return {
      main: entrypoint.file.path,
      emitFunction,
      metadata,
      emitterOutputDir,
      options: emitterOptions,
      library,
    };
  }

  /** TS `createProgram` linter ruleset setup (CLI overrides, then the project config). */
  async function configureLinter(linter: any, resolver: any) {
    const linterOverrides = options.linterRuleSetOverrides;
    const ruleOverrides = new Map<string, unknown>(
      linterOverrides?.rules.map(([ref, value]: [string, unknown]) => [
        resolver.resolveCode(ref),
        value,
      ]),
    );
    const excludeRules = new Set(ruleOverrides.keys());
    if (linterOverrides?.extends !== undefined) {
      reportDiagnostics(
        await linter.extendRuleSet({ extends: linterOverrides.extends }, { excludeRules }),
      );
    }
    if (options.linterRuleSet) {
      const ruleSet =
        linterOverrides?.extends !== undefined
          ? { ...options.linterRuleSet, extends: undefined }
          : options.linterRuleSet;
      reportDiagnostics(
        await linter.extendRuleSet(ruleSet, {
          baseDir: projectRoot,
          source: options.configFile?.file
            ? { script: options.configFile.file, path: ["linter"] }
            : undefined,
          excludeRules,
        }),
      );
    }
    for (const [ref, value] of ruleOverrides) {
      reportDiagnostics(
        await linter.extendRuleSet(
          value === false
            ? { disable: { [ref]: "Disabled via --linter-rule." } }
            : { enable: { [ref]: value } },
        ),
      );
    }
  }
}

/**
 * The compiler's built-in rules (`unused-using`, `unused-template-parameter`) are computed by
 * the Rust checker; the TS linter only decides which are enabled (`create` records the rule).
 */
function wrapBuiltInLinterLibrary(library: any, enabled: string[]) {
  return {
    ...library,
    linter: {
      ...library.linter,
      rules: library.linter.rules.map((rule: any) => ({
        ...rule,
        create() {
          if (!enabled.includes(rule.id)) enabled.push(rule.id);
          return {};
        },
      })),
    },
  };
}

/**
 * Give every root node shim a `TypeSpecScript` parent so TS `getSourceLocation(type)` (used by
 * library linter rules and `getLocationContext`) finds the file instead of a synthetic location.
 */
function attachScriptParents(nodes: Map<number, NodeShim>, getSourceFile: (path: string) => any) {
  const scripts = new Map<string, any>();
  for (const node of nodes.values()) {
    if (node.parent || typeof node.file !== "string") continue;
    let script = scripts.get(node.file);
    if (!script) {
      script = { kind: SyntaxKind.TypeSpecScript, file: getSourceFile(node.file), pos: 0, end: 0 };
      scripts.set(node.file, script);
    }
    node.parent = script;
  }
}

function createSourceFileCache(internals: CompilerInternals, texts: Map<string, string>) {
  const files = new Map<string, any>();
  return (path: string) => {
    let file = files.get(path);
    if (!file) {
      let text = texts.get(path);
      if (text === undefined) {
        try {
          text = readFileSync(path, "utf8");
        } catch {
          text = "";
        }
      }
      file = internals.sourceFile.createSourceFile(text, path);
      files.set(path, file);
    }
    return file;
  };
}

/** `tspconfig.yaml` project settings of `dir` (no `extends`), parsed with the compiler's YAML. */
function readProjectConfig(
  internals: CompilerInternals,
  dir: string,
): ProjectConfigInfo | undefined {
  let text: string;
  try {
    text = readFileSync(joinPaths(dir, "tspconfig.yaml"), "utf8");
  } catch {
    return undefined;
  }
  const [script, diagnostics] = internals.yaml.parseYaml(text);
  const value = script?.value;
  if (diagnostics.length > 0 || !value || typeof value !== "object" || value.kind !== "project") {
    return undefined;
  }
  return {
    entrypoint: typeof value.entrypoint === "string" ? value.entrypoint : undefined,
    features: Array.isArray(value.features) ? value.features : undefined,
  };
}

/** TS `resolveLibraryFeatures` for every resolved library root. */
async function resolveLibraryFeatures(
  host: BridgeCompilerHost,
  internals: CompilerInternals,
  roots: ReadonlySet<string>,
): Promise<{ root: string; features: string[] }[]> {
  const result: { root: string; features: string[] }[] = [];
  for (const root of roots) {
    const config = await internals.configLoader.loadTypeSpecConfigForPath(host, root, false, false);
    if (config.diagnostics.length === 0 && config.kind === "project" && config.features?.length) {
      result.push({ root, features: [...config.features] });
    }
  }
  return result;
}

function readPackageName(root: string): string | undefined {
  try {
    const name = JSON.parse(readFileSync(joinPaths(root, "package.json"), "utf8")).name;
    return typeof name === "string" ? name : undefined;
  } catch {
    return undefined;
  }
}

function findPackageRoot(path: string): string | undefined {
  let dir = getDirectoryPath(normalizePath(path));
  for (;;) {
    try {
      readFileSync(joinPaths(dir, "package.json"));
      return dir;
    } catch {}
    const parent = getDirectoryPath(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

function startTimer(): () => number {
  const start = performance.now();
  return () => performance.now() - start;
}
