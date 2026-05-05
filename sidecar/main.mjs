import readline from 'node:readline';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import {
  configureHost,
  handle,
  handleReverseRpcResponse,
} from './runtime.mjs';

function artifactPath(relativePath) {
  const resolved = path.resolve('/app/artifacts', relativePath);
  if (!resolved.startsWith('/app/artifacts/')) {
    throw new Error(`invalid artifact path: ${relativePath}`);
  }
  return resolved;
}

function denied(op) {
  return Promise.resolve()
    .then(op)
    .then(() => false)
    .catch(() => true);
}

async function nodeConnectDenied(host, port) {
  return denied(
    () =>
      new Promise((resolve, reject) => {
        const socket = net.connect(port, host);
        socket.once('connect', () => {
          socket.destroy();
          resolve();
        });
        socket.once('error', reject);
        socket.setTimeout(1000, () => {
          socket.destroy();
          reject(new Error('socket timeout'));
        });
      }),
  );
}

function respond(response) {
  process.stdout.write(
    `${JSON.stringify(response, (_key, value) =>
      typeof value === 'bigint' ? value.toString() : value,
    )}\n`,
  );
}

configureHost({
  writeLine(line) {
    process.stdout.write(`${line}\n`);
  },
  log(message) {
    process.stderr.write(`${message}\n`);
  },
  readArtifact(relativePath) {
    const resolved = artifactPath(relativePath);
    return fs
      .readFile(resolved, relativePath.endsWith('.json') ? 'utf8' : undefined)
      .catch(() => null);
  },
  async writeArtifact(dir, relativePath, item) {
    await fs.mkdir(artifactPath(dir), { recursive: true });
    await fs.writeFile(artifactPath(relativePath), item);
  },
  artifactExists(relativePath) {
    return fs
      .access(artifactPath(relativePath))
      .then(() => true)
      .catch(() => false);
  },
  async permissionSmoke(params) {
    const nodeNetHost = params.node_net_host ?? '127.0.0.1';
    const nodeNetPort = params.node_net_port ?? 53;
    return {
      fetch_denied: await denied(() => fetch('https://example.com')),
      connect_denied: await nodeConnectDenied('1.1.1.1', 53),
      node_net_denied: await nodeConnectDenied(nodeNetHost, nodeNetPort),
      write_denied: await denied(() =>
        fs.writeFile('/tmp/undercover-deny-write', 'x'),
      ),
      env_denied: !process.env.UNDERCOVER_FORBIDDEN_ENV,
      read_allowed: await denied(() => fs.readFile('/app/artifacts/manifest')).then(
        (isDenied) => !isDenied,
      ),
    };
  },
});

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

rl.on('close', () => {
  process.exit(0);
});

rl.on('line', async (line) => {
  let id = null;
  try {
    const request = JSON.parse(line);
    if (request.undercover_reverse_rpc) {
      if (!handleReverseRpcResponse(request)) {
        throw new Error(`unknown reverse RPC response id: ${request.id}`);
      }
      return;
    }
    id = typeof request.id === 'number' ? request.id : null;
    if (request.jsonrpc !== '2.0' || typeof request.method !== 'string') {
      throw new Error('invalid JSON-RPC request');
    }
    const result = await handle(request.method, request.params);
    respond({ jsonrpc: '2.0', id, result });
  } catch (err) {
    respond({
      jsonrpc: '2.0',
      id,
      error: {
        code: -32603,
        message: err instanceof Error ? err.message : String(err),
        data: err instanceof Error ? err.stack : undefined,
      },
    });
  }
});
