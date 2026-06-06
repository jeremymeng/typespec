import {
  forceGc,
  ITERATIONS,
  MB,
  MODEL_COUNT,
  PROPS_PER_MODEL,
  summarize,
  traverseEager,
  type BenchSample,
} from "./bench-utils.js";
import { native } from "./native.js";

export function runEagerBenchmark(): ReturnType<typeof summarize> {
  const samples: BenchSample[] = [];

  for (let i = 0; i < ITERATIONS; i++) {
    forceGc();
    const before = process.memoryUsage();
    const buildStart = performance.now();
    const tree = native.parseMiniEager(MODEL_COUNT, PROPS_PER_MODEL);
    const buildEnd = performance.now();
    const afterBuild = process.memoryUsage();

    const traversalStart = performance.now();
    const checksum = traverseEager(tree);
    const traversalEnd = performance.now();

    samples.push({
      buildMs: buildEnd - buildStart,
      traversalMs: traversalEnd - traversalStart,
      heapDeltaMb: (afterBuild.heapUsed - before.heapUsed) / MB,
      externalDeltaMb: (afterBuild.external - before.external) / MB,
      nodeCount: tree.nodeCount,
      checksum,
    });
  }

  return summarize(samples);
}
