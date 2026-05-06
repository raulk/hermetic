// Snark proving must run single-threaded inside the embedded Deno worker.
// The SDK's groth16 calls spawn Web Workers by default, which would either
// fail or escape the permission sandbox. This wrapper forces single-thread
// mode and stubs out the Worker global for the duration of each call.

import process from "node:process";
import { groth16 as upstreamGroth16 } from "snarkjs";

import { trace } from "./host-ops.mjs";

let snarkMutationDepth = 0;

async function withSnarkSingleThread(label, operation) {
  const started = Date.now();
  if (snarkMutationDepth !== 0) {
    throw new Error(`nested snark operation is not supported: ${label}`);
  }
  snarkMutationDepth += 1;
  const previousBrowser = process.browser;
  const hadWorker = Object.hasOwn(globalThis, "Worker");
  const previousWorker = globalThis.Worker;
  trace(`snark ${label} start`);
  try {
    process.browser = true;
    globalThis.Worker = undefined;
    const result = await operation();
    trace(`snark ${label} ok ms=${Date.now() - started}`);
    return result;
  } catch (error) {
    trace(`snark ${label} failed ms=${Date.now() - started}`);
    trace(`snark ${label} error ${String(error?.stack ?? error)}`);
    throw error;
  } finally {
    snarkMutationDepth -= 1;
    process.browser = previousBrowser;
    if (hadWorker) {
      globalThis.Worker = previousWorker;
    } else {
      delete globalThis.Worker;
    }
  }
}

export const groth16 = {
  ...upstreamGroth16,
  fullProve(input, wasm, zkey, logger, wtnsCalcOptions, proverOptions) {
    return withSnarkSingleThread("fullProve", () =>
      upstreamGroth16.fullProve(
        input,
        wasm,
        zkey,
        logger,
        { ...(wtnsCalcOptions ?? {}), singleThread: true },
        { ...(proverOptions ?? {}), singleThread: true },
      ));
  },
  verify(...args) {
    return withSnarkSingleThread(
      "verify",
      () => upstreamGroth16.verify(...args),
    );
  },
};
