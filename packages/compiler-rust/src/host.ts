// Multi-file program host for the Rust checker (M9).
//
// The Rust side (`checkProgram` in crates/typespec-napi/src/program.rs) loads
// files depth-first and asks this host to read files and resolve import
// specifiers. Module resolution is a synchronous port of
// `packages/compiler/src/module-resolver/module-resolver.ts` with the options
// used by the TS source loader (`typespec` export condition, `tspMain`, and
// `main.tsp`/`index.mjs`/`index.js` directory index files).

import { readFileSync, realpathSync, statSync } from "node:fs";
import { checkProgram as checkProgramNative, type CheckProgramResultJs } from "../index.js";
import { getDirectoryPath, joinPaths, normalizePath, resolvePath } from "./path-utils.js";

export interface ProgramFileStat {
  isFile(): boolean;
  isDirectory(): boolean;
}

/** Synchronous file-system access used for loading and module resolution. */
export interface ProgramFileSystem {
  /** Returns `undefined` when the file does not exist. */
  readFile(path: string): string | undefined;
  /** Returns `undefined` when the path does not exist. */
  stat(path: string): ProgramFileStat | undefined;
  realpath(path: string): string;
}

export interface ModuleResolution {
  resolvedPath?: string;
  /** Resolved through a package manifest: the loaded file is in a library location context. */
  library?: boolean;
  error?: "not-found" | "invalid";
  message?: string;
  manifestPath?: string;
}

export interface ProgramHost {
  readFile(path: string): string | undefined;
  resolveModule(specifier: string, baseDir: string): ModuleResolution;
  /** Resolve the entry path: a directory resolves to its TypeSpec entrypoint. */
  resolveEntrypoint(path: string): string;
}

type ResolveErrorCode =
  "MODULE_NOT_FOUND" | "INVALID_MODULE" | "INVALID_MAIN" | "INVALID_MODULE_EXPORT_TARGET";

interface NodePackage {
  name?: string;
  main?: unknown;
  tspMain?: string;
  exports?: Exports;
  dir: string;
  file: string;
}

type Exports = null | string | Exports[] | { [key: string]: Exports };

interface ResolvedModulePath {
  path: string;
  library: boolean;
}

class ResolveModuleError extends Error {
  constructor(
    public code: ResolveErrorCode,
    message: string,
    public pkg?: NodePackage,
  ) {
    super(message);
  }
}

class EsmResolveError extends Error {}
class InvalidPackageTargetError extends EsmResolveError {}
class NoMatchingConditionsError extends InvalidPackageTargetError {}

interface EsmContext {
  specifier: string;
  packageUrl: string;
}

function esmErrorMessage(context: EsmContext, reason?: string) {
  return `Could not resolve import "${context.specifier}"  using exports defined in ${context.packageUrl}.${reason ? ` ${reason}` : ""}`;
}

const conditions = ["typespec"];
const directoryIndexFiles = ["main.tsp", "index.mjs", "index.js"];

function pathToFileURL(path: string): string {
  return path.startsWith("//") ? `file:${path}` : `file://${path}`;
}

function fileURLToPath(url: string): string {
  const path = decodeURIComponent(url.slice("file://".length));
  return /^[^/]/.test(path) && !/^[a-zA-Z]:/.test(path) ? `//${path}` : path;
}

function listDirHierarchy(baseDir: string): string[] {
  const paths = [baseDir];
  let current = getDirectoryPath(baseDir);
  while (current !== paths[paths.length - 1]) {
    paths.push(current);
    current = getDirectoryPath(current);
  }
  return paths;
}

function parseNodeModuleSpecifier(id: string): { packageName: string; subPath: string } | null {
  if (id.startsWith(".") || id.startsWith("/")) return null;
  const split = id.split("/");
  if (split[0][0] === "@") {
    return { packageName: `${split[0]}/${split[1]}`, subPath: split.slice(2).join("/") };
  }
  return { packageName: split[0], subPath: split.slice(1).join("/") };
}

function isConditions(item: Exports) {
  return (
    typeof item === "object" && item !== null && Object.keys(item).every((k) => !k.startsWith("."))
  );
}

function resolvePackageTarget(
  context: EsmContext,
  target: Exports,
  patternMatch?: string,
): string | null | undefined {
  if (typeof target === "string") {
    if (!target.startsWith("./")) {
      throw new InvalidPackageTargetError(
        esmErrorMessage(context, `Invalid mapping: "${target}".`),
      );
    }
    const packageUrl = context.packageUrl.endsWith("/")
      ? context.packageUrl
      : `${context.packageUrl}/`;
    const resolved = resolvePath(packageUrl, target);
    if (!resolved.startsWith(context.packageUrl)) {
      throw new InvalidPackageTargetError(
        esmErrorMessage(
          context,
          `Resolved to ${resolved} which is outside package ${context.packageUrl}`,
        ),
      );
    }
    return patternMatch ? resolved.replace(/\*/g, patternMatch) : resolved;
  }
  if (Array.isArray(target)) {
    if (target.length === 0) return null;
    let lastError: unknown = null;
    for (const item of target) {
      try {
        const resolved = resolvePackageTarget(context, item, patternMatch);
        if (resolved !== undefined) return resolved;
      } catch (error) {
        if (!(error instanceof InvalidPackageTargetError)) throw error;
        lastError = error;
      }
    }
    if (lastError) throw lastError;
    return null;
  }
  if (target && typeof target === "object") {
    // `ignoreDefaultCondition` is set by the TS source loader: only `typespec` matches.
    for (const [key, value] of Object.entries(target)) {
      if (conditions.includes(key)) {
        const resolved = resolvePackageTarget(context, value, patternMatch);
        if (resolved !== undefined) return resolved;
      }
    }
    return undefined;
  }
  if (target === null) return null;
  throw new InvalidPackageTargetError(esmErrorMessage(context, `Invalid exports field.`));
}

function resolvePackageExports(
  context: EsmContext,
  subpath: string,
  exports: Exports,
): string | null | undefined {
  if (exports === null) return undefined;
  if (subpath === ".") {
    let mainExport: Exports | undefined;
    if (typeof exports === "string" || Array.isArray(exports) || isConditions(exports)) {
      mainExport = exports;
    } else if ((exports as Record<string, Exports>)["."]) {
      mainExport = (exports as Record<string, Exports>)["."];
    }
    if (mainExport) {
      if (typeof mainExport === "string") return undefined;
      const resolved = resolvePackageTarget(context, mainExport);
      if (resolved) return resolved;
      throw new NoMatchingConditionsError(esmErrorMessage(context, `No conditions matched`));
    }
  } else if (typeof exports === "object" && !isConditions(exports)) {
    const resolved = resolvePackageImportsExports(
      context,
      subpath,
      exports as Record<string, Exports>,
    );
    if (resolved) return resolved;
  }
  throw new EsmResolveError(esmErrorMessage(context));
}

function resolvePackageImportsExports(
  context: EsmContext,
  subpath: string,
  matchObj: Record<string, Exports>,
): string | null | undefined {
  if (!subpath.includes("*") && subpath in matchObj) {
    return resolvePackageTarget(context, matchObj[subpath], "");
  }
  const expansionKeys = Object.keys(matchObj)
    .filter((k) => k.endsWith("/") || k.includes("*"))
    .sort(nodePatternKeyCompare);
  for (const expansionKey of expansionKeys) {
    const star = expansionKey.indexOf("*");
    const patternBase = star === -1 ? expansionKey : expansionKey.substring(0, star);
    if (subpath.startsWith(patternBase) && subpath !== patternBase) {
      const patternTrailer = star !== -1 ? expansionKey.substring(star + 1) : "";
      if (
        patternTrailer.length === 0 ||
        (subpath.endsWith(patternTrailer) && subpath.length >= expansionKey.length)
      ) {
        return resolvePackageTarget(
          context,
          matchObj[expansionKey],
          subpath.substring(patternBase.length, subpath.length - patternTrailer.length),
        );
      }
    }
  }
  throw new EsmResolveError(esmErrorMessage(context));
}

function nodePatternKeyCompare(keyA: string, keyB: string) {
  const baseLengthA = keyA.includes("*") ? keyA.indexOf("*") + 1 : keyA.length;
  const baseLengthB = keyB.includes("*") ? keyB.indexOf("*") + 1 : keyB.length;
  const rval = baseLengthB - baseLengthA;
  if (rval !== 0) return rval;
  if (!keyA.includes("*")) return 1;
  if (!keyB.includes("*")) return -1;
  return keyB.length - keyA.length;
}

/** Create a {@link ProgramHost} over a synchronous file system. */
export function createProgramHost(fs: ProgramFileSystem): ProgramHost {
  const isFile = (path: string) => fs.stat(path)?.isFile() ?? false;
  const realpath = (path: string) => {
    try {
      return normalizePath(fs.realpath(path));
    } catch {
      return path;
    }
  };
  const readPackage = (pkgFile: string): NodePackage => {
    const content = fs.readFile(pkgFile) ?? "";
    return { ...JSON.parse(content), dir: getDirectoryPath(pkgFile), file: pkgFile };
  };

  function resolveModulePath(specifier: string, baseDir: string): ResolvedModulePath {
    const absoluteStart = realpath(resolvePath(baseDir));
    // Set when resolution goes through a package manifest (TS `type: "module"`).
    let library = false;

    const loadAsFile = (file: string): string | undefined => {
      if (isFile(file)) return realpath(file);
      for (const ext of [".mjs", ".js"]) {
        if (isFile(file + ext)) return realpath(file + ext);
      }
      return undefined;
    };

    const loadAsDirectory = (directory: string): string | undefined => {
      const pkgFile = resolvePath(directory, "package.json");
      if (isFile(pkgFile)) {
        const resolved = loadPackage(readPackage(pkgFile));
        if (resolved) return resolved;
      }
      for (const file of directoryIndexFiles) {
        const resolved = loadAsFile(joinPaths(directory, file));
        if (resolved) return resolved;
      }
      return undefined;
    };

    const resolveExports = (subPath: string, pkg: NodePackage): string | undefined => {
      if (!pkg.exports) return undefined;
      const context = { specifier, packageUrl: pathToFileURL(pkg.dir) };
      let match: string | null | undefined;
      try {
        match = resolvePackageExports(context, subPath === "" ? "." : `./${subPath}`, pkg.exports);
      } catch (error) {
        if (error instanceof NoMatchingConditionsError) {
          if (subPath === "") return undefined;
          throw new ResolveModuleError("INVALID_MODULE", error.message, pkg);
        } else if (error instanceof InvalidPackageTargetError) {
          throw new ResolveModuleError("INVALID_MODULE_EXPORT_TARGET", error.message, pkg);
        } else if (error instanceof EsmResolveError) {
          throw new ResolveModuleError("INVALID_MODULE", error.message, pkg);
        }
        throw error;
      }
      if (!match) return undefined;
      const resolved = realpath(fileURLToPath(match));
      if (isFile(resolved)) return resolved;
      throw new ResolveModuleError(
        "INVALID_MODULE_EXPORT_TARGET",
        `Import "${specifier}" resolving to "${resolved}" is not a file.`,
        pkg,
      );
    };

    const loadPackage = (pkg: NodePackage, subPath?: string): string | undefined => {
      const exported = resolveExports(subPath ?? "", pkg);
      if (exported) {
        library = true;
        return exported;
      }
      if (subPath !== undefined && subPath !== "") return undefined;

      const mainFile = pkg.tspMain ?? pkg.main;
      if (mainFile === undefined || mainFile === null) {
        throw new ResolveModuleError(
          "INVALID_MODULE",
          `Package ${pkg.name} is missing a main file or exports field.`,
          pkg,
        );
      }
      if (typeof mainFile !== "string") {
        throw new ResolveModuleError(
          "INVALID_MAIN",
          `Package ${pkg.name} main file "${mainFile}" must be a string.`,
          pkg,
        );
      }
      const mainFullPath = resolvePath(pkg.dir, mainFile);
      let loaded: string | undefined;
      try {
        loaded = loadAsFile(mainFullPath) ?? loadAsDirectory(mainFullPath);
      } catch {
        loaded = undefined;
      }
      if (loaded) {
        library = true;
        return loaded;
      }
      throw new ResolveModuleError(
        "INVALID_MAIN",
        `Package ${pkg.name} main file "${mainFile}" is not pointing to a valid file or directory.`,
        pkg,
      );
    };

    const resolvePackage = (packageName: string): NodePackage | undefined => {
      for (const dir of listDirHierarchy(absoluteStart)) {
        const pkgFile = resolvePath(dir, "package.json");
        if (!isFile(pkgFile)) continue;
        const pkg = readPackage(pkgFile);
        if (pkg.name === packageName) return pkg;
        break;
      }
      for (const dir of listDirHierarchy(absoluteStart)) {
        const pkgFile = resolvePath(joinPaths(dir, "node_modules", packageName), "package.json");
        if (isFile(pkgFile)) return readPackage(pkgFile);
      }
      return undefined;
    };

    if (/^(?:\.\.?(?:\/|$)|\/|([A-Za-z]:)?[/\\])/.test(specifier)) {
      const res = resolvePath(absoluteStart, specifier);
      const resolved = loadAsFile(res) ?? loadAsDirectory(res);
      if (resolved) return { path: resolved, library };
    }

    // Package subpath imports (`#foo`) are not supported yet (see docs/rust-port/plan.md M9).
    const moduleSpecifier = parseNodeModuleSpecifier(specifier);
    if (moduleSpecifier !== null) {
      const pkg = resolvePackage(moduleSpecifier.packageName);
      const resolved = pkg && loadPackage(pkg, moduleSpecifier.subPath);
      if (resolved) return { path: resolved, library };
    }

    throw new ResolveModuleError(
      "MODULE_NOT_FOUND",
      `Cannot find module '${specifier}' from '${baseDir}'`,
    );
  }

  function resolveEntrypointForDir(dir: string): string {
    const pkgFile = resolvePath(dir, "package.json");
    const pkg = isFile(pkgFile) ? readPackage(pkgFile) : undefined;
    if (pkg?.exports) {
      try {
        const match = resolvePackageExports(
          { specifier: ".", packageUrl: pathToFileURL(dir) },
          ".",
          pkg.exports,
        );
        if (match) return fileURLToPath(match);
      } catch (e) {
        if (!(e instanceof NoMatchingConditionsError)) throw e;
      }
    }
    if (pkg?.tspMain !== undefined) return resolvePath(dir, pkg.tspMain);
    return resolvePath(dir, "main.tsp");
  }

  return {
    readFile: (path) => fs.readFile(path),
    resolveModule(specifier, baseDir) {
      try {
        const { path, library } = resolveModulePath(specifier, baseDir);
        // An import resolving to a directory loads that directory's entrypoint.
        const resolvedPath = fs.stat(path)?.isDirectory() ? resolveEntrypointForDir(path) : path;
        return { resolvedPath, library };
      } catch (error) {
        if (!(error instanceof ResolveModuleError)) throw error;
        return {
          error: error.code === "MODULE_NOT_FOUND" ? "not-found" : "invalid",
          message: error.message,
          manifestPath: error.pkg?.file,
        };
      }
    },
    resolveEntrypoint(path) {
      const resolved = resolvePath(path);
      return fs.stat(resolved)?.isDirectory() ? resolveEntrypointForDir(resolved) : resolved;
    },
  };
}

/** Host backed by the real file system. */
export function createNodeProgramHost(): ProgramHost {
  return createProgramHost({
    readFile(path) {
      try {
        return readFileSync(path, "utf8");
      } catch {
        return undefined;
      }
    },
    stat(path) {
      try {
        return statSync(path);
      } catch {
        return undefined;
      }
    },
    realpath: (path) => realpathSync(path),
  });
}

/**
 * Host over in-memory files keyed by absolute path. Directories are implied by
 * file paths; `realpath` is the identity (like the TS test file system).
 */
export function createVirtualProgramHost(
  files: Record<string, string> | ReadonlyMap<string, string>,
): ProgramHost {
  const entries = files instanceof Map ? [...files] : Object.entries(files);
  const fileMap = new Map(entries.map(([path, text]) => [resolvePath(path), text]));
  const directories = new Set<string>();
  for (const path of fileMap.keys()) {
    let dir = getDirectoryPath(path);
    while (!directories.has(dir)) {
      directories.add(dir);
      const parent = getDirectoryPath(dir);
      if (parent === dir) break;
      dir = parent;
    }
  }
  return createProgramHost({
    readFile: (path) => fileMap.get(resolvePath(path)),
    stat(path) {
      const resolved = resolvePath(path);
      const file = fileMap.has(resolved);
      const directory = directories.has(resolved);
      if (!file && !directory) return undefined;
      return { isFile: () => file, isDirectory: () => directory };
    },
    realpath: (path) => path,
  });
}

/** Load, parse and check the program rooted at `entryPath` with the Rust checker. */
export function checkProgram(
  entryPath: string,
  host: ProgramHost = createNodeProgramHost(),
  decorators?: object,
): CheckProgramResultJs {
  return checkProgramNative(
    host.resolveEntrypoint(entryPath),
    (path) => host.readFile(path) ?? null,
    (specifier, baseDir) => host.resolveModule(specifier, baseDir),
    decorators,
  );
}
