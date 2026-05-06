import * as esbuild from "esbuild";
import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const runtimeEntry = path.join(import.meta.dirname, "src/runtime.mjs");
const embeddedDir = path.join(root, "embedded");

const outfile = path.join(embeddedDir, "railgun_runtime.bundle.mjs");

async function patchNativeAddons(outfile) {
  let source = await fs.readFile(outfile, "utf8");
  for (
    const module of [
      "buffer",
      "constants",
      "crypto",
      "events",
      "fs",
      "http",
      "https",
      "os",
      "stream",
      "url",
      "util",
      "zlib",
    ]
  ) {
    source = source.replaceAll(` from "${module}"`, ` from "node:${module}"`);
  }
  source = source.replace(
    /module\.exports = require_api\(\)\(require_node_gyp_build2\(\)\(__dirname\)\);/g,
    'throw new Error("native blake-hash binding disabled for embedded runtime");',
  );
  await fs.writeFile(outfile, source);
}

await fs.mkdir(embeddedDir, { recursive: true });
await esbuild.build({
  entryPoints: [runtimeEntry],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile,
  external: [
    "@railgun-community/poseidon-hash-wasm",
    "@railgun-community/curve25519-scalarmult-wasm",
  ],
});
await patchNativeAddons(outfile);
