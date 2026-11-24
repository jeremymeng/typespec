// M12: output side of the TS `CompilerHost` (`program.host`) used by emitters: backed by the
// Node file system in production and by an in-memory map in tests.

import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { resolvePath } from "./path-utils.js";

/** Output side of TS `CompilerHost` used by emitters (`emitFile`, asset emitters). */
export interface EmitterHost {
  mkdirp(path: string): Promise<string | undefined>;
  writeFile(path: string, content: string): Promise<void>;
  readFile(path: string): Promise<{ text: string; path: string }>;
  rm(path: string, options?: { recursive?: boolean }): Promise<void>;
  stat(path: string): Promise<{ isDirectory(): boolean; isFile(): boolean }>;
  /** TS `logSink`: emitters may log through it. */
  logSink: { log(log: unknown): void };
}

export interface MemoryEmitterHost extends EmitterHost {
  /** Written files by absolute path. */
  readonly files: Map<string, string>;
}

/** Emitter host keeping written files in memory (tests, dry runs). */
export function createMemoryEmitterHost(): MemoryEmitterHost {
  const files = new Map<string, string>();
  const directories = new Set<string>();
  const notFound = (path: string) =>
    Object.assign(new Error(`File not found: ${path}`), { code: "ENOENT" });
  return {
    files,
    async mkdirp(path) {
      directories.add(resolvePath(path));
      return path;
    },
    async writeFile(path, content) {
      files.set(resolvePath(path), content);
    },
    async readFile(path) {
      const text = files.get(resolvePath(path));
      if (text === undefined) throw notFound(path);
      return { text, path };
    },
    async rm(path, options) {
      const resolved = resolvePath(path);
      files.delete(resolved);
      directories.delete(resolved);
      if (options?.recursive) {
        for (const file of [...files.keys()]) {
          if (file.startsWith(resolved + "/")) files.delete(file);
        }
      }
    },
    async stat(path) {
      const resolved = resolvePath(path);
      const isFile = files.has(resolved);
      const isDirectory =
        directories.has(resolved) || [...files.keys()].some((f) => f.startsWith(resolved + "/"));
      if (!isFile && !isDirectory) throw notFound(path);
      return { isFile: () => isFile, isDirectory: () => isDirectory };
    },
    logSink: { log() {} },
  };
}

/** Emitter host writing to the real file system. */
export function createNodeEmitterHost(): EmitterHost {
  return {
    mkdirp: (path) => mkdir(path, { recursive: true }),
    writeFile: (path, content) => writeFile(path, content, "utf8"),
    readFile: async (path) => ({ text: await readFile(path, "utf8"), path }),
    rm: (path, options) => rm(path, { recursive: options?.recursive, force: true }),
    stat: (path) => stat(path),
    logSink: { log() {} },
  };
}
