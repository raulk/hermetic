// Smoke test exercised by the doctor command: each probe must be denied
// by Deno's permission system so the embedded runtime is provably isolated.

import { denoFetch, workdir } from "./host-ops.mjs";

const DENIED_FETCH_PROBE_URL = "https://example.com";

async function denied(op) {
  try {
    await op();
    return false;
  } catch (_) {
    return true;
  }
}

export async function permissionSmoke(params = {}) {
  const net = require("node:net");
  const nodeNetHost = params.node_net_host ?? "127.0.0.1";
  const nodeNetPort = params.node_net_port ?? 53;
  return {
    fetch_denied: await denied(() => denoFetch(DENIED_FETCH_PROBE_URL)),
    connect_denied: await denied(() =>
      Deno.connect({ hostname: "1.1.1.1", port: 53 })
    ),
    node_net_denied: await denied(
      () =>
        new Promise((resolve, reject) => {
          const socket = net.connect(nodeNetPort, nodeNetHost);
          socket.once("connect", () => {
            socket.destroy();
            resolve();
          });
          socket.once("error", reject);
          socket.setTimeout(1000, () => {
            socket.destroy();
            reject(new Error("socket timeout"));
          });
        }),
    ),
    write_denied: await denied(() =>
      Deno.writeTextFile("/tmp/hermetic-deny-write", "x")
    ),
    env_denied: await denied(() => Deno.env.get("HERMETIC_FORBIDDEN_ENV")),
    read_allowed:
      !(await denied(() => Deno.readTextFile(`${workdir}/artifacts/manifest`))),
  };
}
