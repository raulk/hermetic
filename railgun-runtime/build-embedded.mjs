import * as esbuild from 'esbuild';
import fs from 'node:fs/promises';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const runtimeEntry = path.join(import.meta.dirname, 'runtime.mjs');
const embeddedDir = path.join(root, 'embedded');

const outputs = [
  {
    format: 'esm',
    outfile: path.join(embeddedDir, 'railgun_runtime.bundle.mjs'),
  },
  {
    format: 'iife',
    globalName: 'UndercoverRailgunRuntime',
    outfile: path.join(embeddedDir, 'railgun_runtime.iife.js'),
  },
];

async function patchNativeAddons(outfile) {
  let source = await fs.readFile(outfile, 'utf8');
  source = source.replace(
    /module\.exports = require_api\(\)\(require_node_gyp_build2\(\)\(__dirname\)\);/g,
    'throw new Error("native blake-hash binding disabled for embedded runtime");',
  );
  await fs.writeFile(outfile, source);
}

await fs.mkdir(embeddedDir, { recursive: true });
for (const output of outputs) {
  await esbuild.build({
    entryPoints: [runtimeEntry],
    bundle: true,
    platform: 'node',
    format: output.format,
    globalName: output.globalName,
    outfile: output.outfile,
    external: [
      '@railgun-community/poseidon-hash-wasm',
      '@railgun-community/curve25519-scalarmult-wasm',
    ],
  });
  await patchNativeAddons(output.outfile);
}
