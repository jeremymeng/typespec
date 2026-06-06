import { ok } from "node:assert";
import { describe, it } from "vitest";
import { runEagerBenchmark } from "./eager.bench.js";
import { runLazyBenchmark } from "./lazy.bench.js";
import { isSpikeNativeBuilt } from "./native.js";

function ratio(a: number, b: number): number {
  if (b === 0) return Number.POSITIVE_INFINITY;
  return Math.round((a / b) * 1000) / 1000;
}

const describeIfSpikeNativeBuilt = isSpikeNativeBuilt ? describe : describe.skip;

describeIfSpikeNativeBuilt("spike eager vs lazy round-trip benchmark", () => {
  it("measures 10 iterations of both napi-rs AST transfer strategies", () => {
    const eager = runEagerBenchmark();
    const lazy = runLazyBenchmark();
    const comparison = {
      nodeCount: eager.nodeCount,
      eager,
      lazy,
      ratios: {
        buildEagerToLazy: ratio(eager.buildMedianMs, lazy.buildMedianMs),
        traversalLazyToEager: ratio(lazy.traversalMedianMs, eager.traversalMedianMs),
        heapEagerToLazy: ratio(eager.heapDeltaMedianMb, lazy.heapDeltaMedianMb),
      },
    };

    console.log("M1.5 round-trip benchmark", JSON.stringify(comparison, null, 2));
    ok(eager.nodeCount >= 10_000);
    ok(lazy.nodeCount === eager.nodeCount);
    ok(eager.checksum === lazy.checksum);
  });
});
