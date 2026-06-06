import { ok, strictEqual } from "node:assert";
import { Worker } from "node:worker_threads";
import { describe, it } from "vitest";
import { forceGc, MODEL_COUNT, PROPS_PER_MODEL } from "./bench-utils.js";
import { isSpikeNativeBuilt, native } from "./native.js";

function tick(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

const describeIfSpikeNativeBuilt = isSpikeNativeBuilt ? describe : describe.skip;

describeIfSpikeNativeBuilt("spike External<MiniArena> lifetime", () => {
  it("keeps the arena alive while JS retains the External handle", () => {
    native.resetDropCount();
    const handle = native.parseMiniLazy(MODEL_COUNT, PROPS_PER_MODEL);
    const stats = native.getArenaStats(handle);
    const allocations: Array<{ index: number; text: string }> = [];

    for (let i = 0; i < 1_000_000; i++) {
      allocations.push({ index: i, text: `dummy-${i}` });
    }

    strictEqual(native.getDropCount(), 0);
    strictEqual(native.getNode(handle, stats.root).kind, "Module");
    ok(allocations.length === 1_000_000);
  });

  it("finalizes the arena after the JS handle becomes unreachable when GC is exposed", async () => {
    native.resetDropCount();
    let handle: unknown = native.parseMiniLazy(MODEL_COUNT, PROPS_PER_MODEL);
    const weak = new WeakRef(handle as object);
    strictEqual(native.getDropCount(), 0);

    handle = null;
    for (let i = 0; i < 20; i++) {
      forceGc();
      await tick();
    }

    const recovered = weak.deref();
    if (recovered === undefined) {
      ok(native.getDropCount() >= 1);
    } else {
      // GC is non-deterministic. If V8 has not collected it, the handle is still valid.
      strictEqual(native.getNode(recovered, 0).kind, "Identifier");
      strictEqual(native.getDropCount(), 0);
    }
  });

  it("rejects an External with the wrong Rust type tag", () => {
    const wrong = native.parseWrongExternal();
    try {
      native.getNode(wrong, 0);
      throw new Error("getNode unexpectedly accepted the wrong External type");
    } catch (error) {
      ok(String(error).includes("not the type of wrapped object"));
    }
  });

  it("cannot post the External handle to a worker thread on Node", async () => {
    const handle = native.parseMiniLazy(1, 1);
    const worker = new Worker(
      `const { parentPort } = require("node:worker_threads"); parentPort.on("message", () => parentPort.postMessage("received"));`,
      { eval: true },
    );

    let threw = false;
    try {
      worker.postMessage(handle);
    } catch (error) {
      threw = true;
      ok(String(error).includes("could not be cloned"));
    } finally {
      await worker.terminate();
    }
    ok(threw, "External handle unexpectedly crossed the worker boundary");
  });
});
