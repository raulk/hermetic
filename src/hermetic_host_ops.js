import { createRequire } from "node:module";
import {
  op_hermetic_artifact_exists,
  op_hermetic_log,
  op_hermetic_progress,
  op_hermetic_read_artifact,
  op_hermetic_reverse_request,
  op_hermetic_service_endpoint,
  op_hermetic_workdir,
  op_hermetic_write_artifact,
} from "ext:core/ops";

const workdir = op_hermetic_workdir();

globalThis.__hermetic_workdir = workdir;
globalThis.__dirname = `${workdir}/embedded`;
globalThis.__filename = `${globalThis.__dirname}/railgun_runtime.bundle.mjs`;
globalThis.__hermetic_deno_fetch = globalThis.fetch;
globalThis.require = createRequire(
  `file://${workdir}/railgun-runtime/runtime.mjs`,
);

globalThis.__hermetic_ops = {
  op_hermetic_artifact_exists,
  op_hermetic_log,
  op_hermetic_progress,
  op_hermetic_read_artifact,
  op_hermetic_service_endpoint,
  op_hermetic_reverse_request,
  op_hermetic_write_artifact,
};
