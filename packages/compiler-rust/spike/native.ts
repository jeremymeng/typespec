import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export interface NodeInfo {
  id: number;
  kind: string;
  children: number[];
  name?: number;
  typeRef?: number;
  text?: string;
}

export interface SerializedTree {
  root: number;
  nodeCount: number;
  nodes: NodeInfo[];
}

export interface ArenaStats {
  root: number;
  nodeCount: number;
}

export interface SpikeNative {
  parseMiniEager(modelCount?: number, propsPerModel?: number): SerializedTree;
  parseMiniLazy(modelCount?: number, propsPerModel?: number): unknown;
  getNode(handle: unknown, id: number): NodeInfo;
  getArenaStats(handle: unknown): ArenaStats;
  getDropCount(): number;
  resetDropCount(): void;
  parseWrongExternal(): unknown;
}

const require = createRequire(import.meta.url);
const spikeDir = dirname(fileURLToPath(import.meta.url));
const nativeFile = readdirSync(spikeDir).find(
  (file) => file.startsWith("spike-napi-roundtrip.") && file.endsWith(".node"),
);

let loadedNative: SpikeNative | undefined;

export const isSpikeNativeBuilt = nativeFile !== undefined;

export function loadNative(): SpikeNative {
  if (loadedNative !== undefined) return loadedNative;
  if (nativeFile === undefined) {
    throw new Error(
      `Missing spike native module in ${spikeDir}. Run pnpm -C packages/compiler-rust build:spike first.`,
    );
  }

  loadedNative = require(join(spikeDir, nativeFile)) as SpikeNative;
  return loadedNative;
}

export const native = new Proxy({} as SpikeNative, {
  get(_target, property) {
    return loadNative()[property as keyof SpikeNative];
  },
});
