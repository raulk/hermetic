import * as wallet from "@railgun-community/wallet";

import {
  op_hermetic_artifact_exists,
  op_hermetic_read_artifact,
  op_hermetic_write_artifact,
  trace,
} from "./host-ops.mjs";

function isJsonArtifact(relativePath) {
  return relativePath.toLowerCase().endsWith(".json");
}

function artifactBytes(item) {
  if (typeof item === "string") {
    return new TextEncoder().encode(item);
  }
  return item;
}

export function randomHexPrivateKey() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return `0x${
    Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")
  }`;
}

export const artifactStore = new wallet.ArtifactStore(
  (relativePath) => {
    const started = Date.now();
    const item = op_hermetic_artifact_exists(relativePath)
      ? op_hermetic_read_artifact(relativePath)
      : null;
    const artifact = item != null && isJsonArtifact(relativePath)
      ? new TextDecoder().decode(item)
      : item;
    trace(
      `artifact read path=${relativePath} result=${
        artifact?.byteLength ?? artifact?.length ?? "null"
      } ms=${Date.now() - started}`,
    );
    return artifact;
  },
  (dir, relativePath, item) => {
    trace(
      `artifact write path=${relativePath} item=${
        item?.byteLength ?? item?.length ?? "null"
      }`,
    );
    return op_hermetic_write_artifact(dir, relativePath, artifactBytes(item));
  },
  (relativePath) => {
    const exists = op_hermetic_artifact_exists(relativePath);
    trace(`artifact exists path=${relativePath} result=${exists}`);
    return exists;
  },
);
