import { native, type SerializedTree } from "./native.js";

export const MODEL_COUNT = 834;
export const PROPS_PER_MODEL = 5;
export const ITERATIONS = 10;
export const MB = 1024 * 1024;

export interface BenchSample {
  buildMs: number;
  traversalMs: number;
  heapDeltaMb: number;
  externalDeltaMb: number;
  nodeCount: number;
  checksum: number;
}

export interface BenchSummary {
  buildMedianMs: number;
  buildP95Ms: number;
  traversalMedianMs: number;
  traversalP95Ms: number;
  heapDeltaMedianMb: number;
  externalDeltaMedianMb: number;
  nodeCount: number;
  checksum: number;
  samples: BenchSample[];
}

export function forceGc(): void {
  const gc = (globalThis as typeof globalThis & { gc?: () => void }).gc;
  for (let i = 0; i < 5; i++) {
    gc?.();
  }
}

export function traverseEager(tree: SerializedTree): number {
  let checksum = 0;
  for (const node of tree.nodes) {
    checksum += node.id + node.children.length;
    if (node.name !== undefined) checksum += node.name;
    if (node.typeRef !== undefined) checksum += node.typeRef;
    if (node.text !== undefined) checksum += node.text.length;
  }
  return checksum;
}

export function traverseLazy(handle: unknown): { checksum: number; nodeCount: number } {
  const stats = native.getArenaStats(handle);
  let checksum = 0;
  for (let id = 0; id < stats.nodeCount; id++) {
    const node = native.getNode(handle, id);
    checksum += node.id + node.children.length;
    if (node.name !== undefined) checksum += node.name;
    if (node.typeRef !== undefined) checksum += node.typeRef;
    if (node.text !== undefined) checksum += node.text.length;
  }
  return { checksum, nodeCount: stats.nodeCount };
}

export function summarize(samples: BenchSample[]): BenchSummary {
  return {
    buildMedianMs: percentile(
      samples.map((sample) => sample.buildMs),
      0.5,
    ),
    buildP95Ms: percentile(
      samples.map((sample) => sample.buildMs),
      0.95,
    ),
    traversalMedianMs: percentile(
      samples.map((sample) => sample.traversalMs),
      0.5,
    ),
    traversalP95Ms: percentile(
      samples.map((sample) => sample.traversalMs),
      0.95,
    ),
    heapDeltaMedianMb: percentile(
      samples.map((sample) => sample.heapDeltaMb),
      0.5,
    ),
    externalDeltaMedianMb: percentile(
      samples.map((sample) => sample.externalDeltaMb),
      0.5,
    ),
    nodeCount: samples[0]?.nodeCount ?? 0,
    checksum: samples[0]?.checksum ?? 0,
    samples,
  };
}

function percentile(values: number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.ceil(sorted.length * p) - 1);
  return round(sorted[index] ?? 0);
}

export function round(value: number): number {
  return Math.round(value * 1000) / 1000;
}
