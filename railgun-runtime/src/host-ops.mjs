// Single import point for the host op surface. The bootstrap shim in
// src/embedded/bootstrap.js stashes the raw ops on globalThis; this file
// destructures them once and re-exports named values so other modules can
// `import { op_hermetic_log } from "./host-ops.mjs"` instead of reaching
// into a global object.

const {
  op_hermetic_artifact_exists,
  op_hermetic_log,
  op_hermetic_progress,
  op_hermetic_read_artifact,
  op_hermetic_service_endpoint,
  op_hermetic_reverse_request,
  op_hermetic_write_artifact,
} = globalThis.__hermetic_ops;

export {
  op_hermetic_artifact_exists,
  op_hermetic_log,
  op_hermetic_progress,
  op_hermetic_read_artifact,
  op_hermetic_reverse_request,
  op_hermetic_service_endpoint,
  op_hermetic_write_artifact,
};

export const workdir = globalThis.__hermetic_workdir;
export const denoFetch = globalThis.__hermetic_deno_fetch;

export function trace(message) {
  op_hermetic_log(`[hermetic-runtime] ${message}`);
}
