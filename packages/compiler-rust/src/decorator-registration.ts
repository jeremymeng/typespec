// M11: run the real JS decorator implementations (`@typespec/compiler`, `@typespec/http`, ...)
// over the Rust-checked program.
//
// The Rust checker validates every decorator application (target, arity, argument
// types/values; `decorator-wrong-target`, `invalid-argument`, ...) and returns them in TS
// execution order (`decoratorApplications`). This module replays them against the Program
// adapter after checking: implementations are loaded from the program's JS source files
// (`$decorators` / `$functions` / legacy `$name` exports, bound like TS `bindJsSourceFile`)
// and called with a TS-shaped `DecoratorContext`. Decorator state goes through the adapter's
// real `program.stateMap` / `program.stateSet`, so the library getters (`getDoc`,
// `getRoutePath`, ...) work unchanged against the adapter. See docs/rust-port/plan.md (M11)
// for why this deviates from the M6 flat `setProperty` bridge.

import { Numeric } from "@typespec/compiler";
import { pathToFileURL } from "node:url";
import type {
  CheckProgramResultJs,
  DecoratorApplicationJs,
  DecoratorArgumentJs,
  ExternDeclarationJs,
} from "../index.js";
import type { Diagnostic } from "./diagnostics.js";
import { loadEmitters, runEmitters, type EmitCompilerOptions } from "./emitter.js";
import { createNodeEmitterHost, type EmitterHost } from "./host-fs.js";
import {
  checkProgram,
  createNodeProgramHost,
  getCompilerRoot,
  type CheckProgramOptions,
  type ProgramHost,
} from "./host.js";
import { joinPaths } from "./path-utils.js";
import {
  createProgram,
  type DecoratorApplication,
  type DecoratorArgument,
  type NodeShim,
  type Program,
  type Type,
} from "./program.js";
import { createValue, type SerializedValue, type ValueShim } from "./values.js";

export type DecoratorImplementation = (context: any, target: any, ...args: any[]) => any;

/** Decorator and function implementations by fully qualified name (`TypeSpec.@doc`). */
export interface ImplementationRegistry {
  decorators: Map<string, DecoratorImplementation>;
  functions: Map<string, (...args: any[]) => any>;
  /** Library `$onValidate` callbacks, in load order. */
  validators: Array<(program: any) => unknown>;
}

export function createImplementationRegistry(): ImplementationRegistry {
  return { decorators: new Map(), functions: new Map(), validators: [] };
}

/** JS exports bound by the TS binder but not decorators. */
const nonDecoratorFunctions = new Set(["onValidate", "provideTypeInfo", "onEmit"]);

/**
 * Register a library's JS module exports by qualified name, like TS `bindJsSourceFile`:
 * `$decorators` / `$functions` namespace maps and legacy `$name` function exports (namespaced
 * by the module's `namespace` export and the function's own `namespace` property).
 */
export function registerLibraryDecorators(
  registry: ImplementationRegistry,
  exports: Record<string, unknown>,
): void {
  const rootNs = typeof exports.namespace === "string" ? exports.namespace : undefined;
  for (const [key, member] of Object.entries(exports)) {
    if (key === "$decorators" && member && typeof member === "object") {
      for (const [ns, decorators] of Object.entries(member as Record<string, object>)) {
        for (const [name, fn] of Object.entries(decorators)) {
          registry.decorators.set(qualify(ns, `@${name}`), fn as DecoratorImplementation);
        }
      }
    } else if (key === "$functions" && member && typeof member === "object") {
      for (const [ns, functions] of Object.entries(member as Record<string, object>)) {
        for (const [name, fn] of Object.entries(functions)) {
          registry.functions.set(qualify(ns, name), fn as (...args: any[]) => any);
        }
      }
    } else if (key === "$onValidate" && typeof member === "function") {
      registry.validators.push(member as (program: any) => unknown);
    } else if (typeof member === "function" && key.startsWith("$")) {
      const name = key.slice(1);
      if (nonDecoratorFunctions.has(name)) continue;
      const memberNs = (member as { namespace?: string }).namespace;
      const ns = [rootNs, memberNs].filter((part) => part).join(".");
      registry.decorators.set(qualify(ns, `@${name}`), member as DecoratorImplementation);
    }
  }
}

function qualify(ns: string, name: string): string {
  return ns === "" ? name : `${ns}.${name}`;
}

export interface CompileOptions extends Omit<CheckProgramOptions, "collectDecoratorApplications"> {
  /**
   * Run the compiler's built-in validators and the libraries' `$onValidate` callbacks after
   * the decorators, like TS `compile` (default `true`).
   */
  runValidators?: boolean;
  /**
   * Module exports for JS files that are not on disk (virtual hosts), keyed by absolute path.
   * Other JS source files are imported from disk.
   */
  jsModules?: Record<string, Record<string, unknown>>;
  /**
   * TS compiler options (M12): `emit` emitters run after validation, like TS `compile`, and
   * become `program.compilerOptions`.
   */
  compilerOptions?: EmitCompilerOptions;
  /** Host emitters write through (`program.host`); defaults to the real file system (M12). */
  outputHost?: EmitterHost;
}

/**
 * Check `entryPath` with the Rust checker and run the decorator implementations of every JS
 * file the program imports (TS `compile` without emitters, linters or `$onValidate`).
 */
export async function compile(
  entryPath: string,
  host: ProgramHost = createNodeProgramHost(),
  options: CompileOptions = {},
): Promise<Program> {
  const {
    jsModules,
    runValidators = true,
    compilerOptions = {},
    outputHost,
    ...checkOptions
  } = options;
  const result = checkProgram(entryPath, host, undefined, {
    ...checkOptions,
    collectDecoratorApplications: true,
  });
  const registry = createImplementationRegistry();
  let allLoaded = true;
  for (const file of result.sourceFiles) {
    if (file.kind !== "js") continue;
    const exports = jsModules?.[file.path] ?? (await importJsFile(file.path));
    if (exports) {
      registerLibraryDecorators(registry, exports);
    } else {
      allLoaded = false;
    }
  }
  const program = createProgram(result);
  program.compilerOptions = { ...compilerOptions };
  program.host = outputHost ?? createNodeEmitterHost();
  // TS loads emitters (and validates their options) before checking.
  const emitters = await loadEmitters(program, compilerOptions, jsModules);
  runDecorators(program, result, registry, { reportMissingImplementations: allLoaded });
  if (runValidators) {
    if (checkOptions.stdlib ?? true) await runCompilerValidators(program);
    for (const validator of registry.validators) {
      const diagnostics = await validator(program);
      if (Array.isArray(diagnostics)) program.reportDiagnostics(diagnostics);
    }
  }
  // Like TS, emitters only run when checking and validation reported no errors.
  if (emitters.length > 0 && !program.hasError()) {
    await runEmitters(program, emitters, compilerOptions);
  }
  return program;
}

/**
 * TS checker `internalDecoratorValidation` and program `runCompilerValidators`. They are not
 * part of the public API, so they are loaded from the same compiler `dist` files the standard
 * library's decorators come from (shared state keys).
 */
async function runCompilerValidators(program: Program): Promise<void> {
  const root = getCompilerRoot();
  const load = (path: string) =>
    import(pathToFileURL(joinPaths(root, path)).href) as Promise<Record<string, any>>;
  const [discriminators, encodedNames, paging] = await Promise.all([
    load("dist/src/core/helpers/discriminator-utils.js"),
    load("dist/src/lib/encoded-names.js"),
    load("dist/src/lib/paging.js"),
  ]);
  discriminators.validateInheritanceDiscriminatedUnions(program);
  encodedNames.validateEncodedNamesConflicts(program);
  paging.validatePagingOperations(program);
}

async function importJsFile(path: string): Promise<Record<string, unknown> | undefined> {
  try {
    return (await import(pathToFileURL(path).href)) as Record<string, unknown>;
  } catch {
    return undefined;
  }
}

export interface RunDecoratorsOptions {
  /** Report `missing-implementation` for extern declarations without a registered implementation. */
  reportMissingImplementations?: boolean;
}

type ValidatorFn = () => readonly Diagnostic[];

/** Replay the validated decorator applications of `result` on `program` (created from it). */
export function runDecorators(
  program: Program,
  result: CheckProgramResultJs,
  registry: ImplementationRegistry,
  options: RunDecoratorsOptions = {},
): void {
  const types = program._typeCache;
  const getNode = (id: number | undefined | null): NodeShim | undefined =>
    id === undefined || id === null ? undefined : program._nodes.get(id);

  const externs = new Map<string, ExternDeclarationJs>();
  for (const declaration of result.externDeclarations) {
    externs.set(declaration.fqn, declaration);
    if (declaration.isAuto || !options.reportMissingImplementations) continue;
    const registered =
      declaration.kind === "decorator"
        ? registry.decorators.has(declaration.fqn)
        : registry.functions.has(declaration.fqn);
    if (!registered) {
      program.reportDiagnostic({
        code: "missing-implementation",
        severity: "error",
        message: "Extern declaration must have an implementation in JS file.",
        target: getNode(declaration.nodeId),
      });
    }
  }

  const autoImplementations = new Map<string, DecoratorImplementation>();
  const resolveImplementation = (fqn: string): DecoratorImplementation | undefined => {
    const registered = registry.decorators.get(fqn);
    if (registered) return registered;
    const declaration = externs.get(fqn);
    if (!declaration?.isAuto) return undefined;
    let implementation = autoImplementations.get(fqn);
    if (!implementation) {
      implementation = createAutoDecoratorImplementation(fqn, declaration);
      autoImplementations.set(fqn, implementation);
    }
    return implementation;
  };

  // Build every type's `decorators` first: implementations inspect them
  // (`validateDecoratorUniqueOnNode`, `validateDecoratorNotOnType`).
  const applicationsByTarget = new Map<
    Type,
    Array<{ app: DecoratorApplication; js: DecoratorApplicationJs }>
  >();
  for (const type of types.values()) {
    type.decorators = [];
  }
  for (const js of result.decoratorApplications) {
    const target = types.get(js.targetId);
    const implementation = resolveImplementation(js.decorator);
    if (!target || !implementation) continue;
    const args = js.args.map((arg) => createArgument(program, arg, getNode));
    if (args.some((arg) => arg === undefined)) continue;
    const node = js.kind === "docComment" ? undefined : getNode(js.nodeId);
    if (node && js.kind === "decorator" && target.node) node.parent = target.node;
    const app: DecoratorApplication = {
      decorator: implementation,
      decoratorName: js.decorator,
      args: args as DecoratorArgument[],
      node,
    };
    target.decorators.push(app);
    let list = applicationsByTarget.get(target);
    if (!list) applicationsByTarget.set(target, (list = []));
    list.push({ app, js });
  }

  // TS `finishType`: run a type's decorators, then its `onTargetFinish` validators; the
  // `onGraphFinish` validators run once the whole program is checked.
  const postCheckValidators: ValidatorFn[] = [];
  for (const [target, applications] of applicationsByTarget) {
    const postSelfValidators: ValidatorFn[] = [];
    for (const { app } of applications) {
      const validators = applyDecorator(program, app, target);
      if (validators?.onTargetFinish) postSelfValidators.push(validators.onTargetFinish);
      if (validators?.onGraphFinish) postCheckValidators.push(validators.onGraphFinish);
    }
    for (const validator of postSelfValidators) program.reportDiagnostics(validator());
  }
  for (const validator of postCheckValidators) program.reportDiagnostics(validator());

  // Types synthesized later (typekits, `filterModelProperties`) run their decorators in
  // `checker.finishType`; their `onGraphFinish` validators are not run (the graph is done).
  program._applyDecoratorsToType = (type) => {
    const postSelfValidators: ValidatorFn[] = [];
    for (const app of (type as { decorators: DecoratorApplication[] }).decorators) {
      if (typeof app.decorator !== "function") continue;
      const validators = applyDecorator(program, app, type);
      if (validators?.onTargetFinish) postSelfValidators.push(validators.onTargetFinish);
    }
    for (const validator of postSelfValidators) program.reportDiagnostics(validator());
  };
}

function applyDecorator(
  program: Program,
  app: DecoratorApplication,
  target: Type,
): { onTargetFinish?: ValidatorFn; onGraphFinish?: ValidatorFn } | void {
  const args = (app.args as DecoratorArgument[]).map((arg) => arg.jsValue);
  const fn = app.decorator as DecoratorImplementation;
  return fn(createDecoratorContext(program, app), target, ...args);
}

/** TS `createDecoratorContext`. */
function createDecoratorContext(program: Program, app: DecoratorApplication) {
  const passthrough = createPassThroughContext(program, app.node);
  const context = {
    program,
    decoratorTarget: app.node,
    getArgumentTarget: (index: number) => (app.args as DecoratorArgument[])[index]?.node,
    call: (decorator: DecoratorImplementation, target: Type, ...args: unknown[]) =>
      context.callDecorator(decorator, target, ...args),
    callDecorator: (decorator: DecoratorImplementation, target: Type, ...args: unknown[]) =>
      decorator(passthrough, target, ...args),
    callFunction: (fn: (...args: any[]) => unknown, ...args: unknown[]) => fn(passthrough, ...args),
  };
  return context;
}

function createPassThroughContext(program: Program, target: NodeShim | undefined) {
  const context: any = {
    program,
    decoratorTarget: target,
    functionCallTarget: target,
    getArgumentTarget: () => target,
    call: (decorator: DecoratorImplementation, type: Type, ...args: unknown[]) =>
      decorator(context, type, ...args),
    callDecorator: (decorator: DecoratorImplementation, type: Type, ...args: unknown[]) =>
      decorator(context, type, ...args),
    callFunction: (fn: (...args: any[]) => unknown, ...args: unknown[]) => fn(context, ...args),
  };
  return context;
}

/**
 * TS `auto dec` implementation (upstream `4f8e31935`): records the named arguments in
 * `program.stateMap(Symbol.for("dec:<fqn>"))`, read by `getAutoDecoratorValue`.
 */
function createAutoDecoratorImplementation(
  fqn: string,
  declaration: ExternDeclarationJs,
): DecoratorImplementation {
  const key = Symbol.for(`dec:${fqn}`);
  return (context: { program: Program }, target: Type, ...args: unknown[]) => {
    const names = declaration.parameterNames;
    const value: Record<string, unknown> = {};
    names.forEach((name, index) => {
      if (declaration.lastParameterRest && index === names.length - 1) {
        value[name] = args.slice(index);
      } else if (index < args.length) {
        value[name] = args[index];
      }
    });
    context.program.stateMap(key).set(target, value);
  };
}

/** Marshal a validated argument like TS `checkDecoratorArgument` + `marshalTypeForJs`. */
function createArgument(
  program: Program,
  arg: DecoratorArgumentJs,
  getNode: (id: number | undefined | null) => NodeShim | undefined,
): DecoratorArgument | undefined {
  const node = getNode(arg.nodeId);
  if (arg.kind === "type") {
    const type =
      arg.typeId === undefined || arg.typeId === null
        ? undefined
        : program._typeCache.get(arg.typeId);
    if (!type) return undefined;
    return { value: type, jsValue: type, node };
  }
  const serialized = JSON.parse(arg.valueJson ?? "null") as SerializedValue | null;
  if (!serialized) return undefined;
  const constraints = arg.valueConstraintIds
    .map((id) => program._typeCache.get(id))
    .filter((type): type is Type => type !== undefined);
  const value = createValue(program, serialized, node);
  if (value === undefined) return undefined;
  const jsValue = marshalValue(value, constraints);
  if (jsValue === unsupported) return undefined;
  return { value, jsValue, node };
}

const unsupported = Symbol("unsupported");

/** TS `marshalTypeForJs`. */
function marshalValue(value: ValueShim, constraints: readonly Type[]): unknown {
  switch (value.valueKind) {
    case "StringValue":
    case "BooleanValue":
      return value.value;
    case "NumericValue": {
      const numeric = value.value as Numeric;
      if (constraints.every(canNumericConstraintBeJsNumber)) {
        const asNumber = numeric.asNumber();
        return asNumber === null ? unsupported : asNumber;
      }
      return numeric;
    }
    case "ObjectValue": {
      const properties = (value as unknown as { properties: Map<string, { value: ValueShim }> })
        .properties;
      return Object.fromEntries(
        [...properties].map(([key, property]) => [key, marshalValue(property.value, [])]),
      );
    }
    case "ArrayValue":
      return (value as unknown as { values: ValueShim[] }).values.map((item) =>
        marshalValue(item, []),
      );
    case "NullValue":
      return null;
    default:
      return value;
  }
}

/** JS-number-representable numeric scalars (TS `numericRanges[name][2].isJsNumber`). */
const jsNumberScalars = new Set([
  "int8",
  "int16",
  "int32",
  "uint8",
  "uint16",
  "uint32",
  "safeint",
  "float32",
  "float64",
]);

/** TS `canNumericConstraintBeJsNumber`. */
function canNumericConstraintBeJsNumber(type: Type): boolean {
  // The adapter keeps Rust alias types (`alias RangeLimitableTypes = numeric | ...`).
  while (type.kind === "Alias") type = type.target;
  switch (type.kind) {
    case "Scalar": {
      let isNumeric = false;
      for (let current: Type | undefined = type; current;) {
        if (
          current.kind === "Scalar" &&
          current.name === "numeric" &&
          current.namespace?.name === "TypeSpec"
        ) {
          isNumeric = true;
          break;
        }
        current = current.kind === "Scalar" ? current.baseScalar : undefined;
      }
      return isNumeric ? jsNumberScalars.has(type.name) : true;
    }
    case "Union":
      return [...type.variants.values()].every((variant) =>
        canNumericConstraintBeJsNumber(variant.type),
      );
    default:
      return true;
  }
}
