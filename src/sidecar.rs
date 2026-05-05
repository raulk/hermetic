use std::{path::Path, process::Stdio};

use anyhow::{anyhow, Context as _};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::{arti::ArtiClient, rpc};

pub struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

#[derive(Debug, Serialize)]
struct RpcRequest<'a, T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: T,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    id: u64,
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct ReverseRpcRequest {
    undercover_reverse_rpc: bool,
    id: u64,
    method: String,
    params: Value,
}

#[derive(Debug, Serialize)]
struct ReverseRpcResponse {
    undercover_reverse_rpc: bool,
    id: u64,
    result: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub sdk_version: String,
    pub shared_models_version: String,
    pub node_compat: bool,
}

#[derive(Debug, Deserialize)]
pub struct PermissionSmoke {
    pub fetch_denied: bool,
    pub connect_denied: bool,
    pub node_net_denied: bool,
    pub write_denied: bool,
    pub env_denied: bool,
    pub read_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoadedWallet {
    pub wallet_id: String,
    pub shielded_address: String,
}

#[derive(Debug, Deserialize)]
pub struct PopulatedTransaction {
    pub to: String,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshedBalance {
    pub token_address: String,
    pub balance: String,
    pub spendable_balance: String,
}

impl Sidecar {
    pub async fn spawn(workdir: &Path) -> anyhow::Result<Self> {
        let mut command = sidecar_command(workdir)?;
        let mut child = command
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning Node sidecar container")?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("sidecar stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("sidecar stdout was not piped"))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub async fn call<Req, Res>(&mut self, method: &str, params: Req) -> anyhow::Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.call_inner(method, params, None).await
    }

    pub async fn call_with_reverse_rpc<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        tor: ArtiClient,
        rpc_url: http::Uri,
    ) -> anyhow::Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.call_inner(method, params, Some((tor, rpc_url))).await
    }

    async fn call_inner<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        reverse_rpc: Option<(ArtiClient, http::Uri)>,
    ) -> anyhow::Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        let id = self.next_id;
        self.next_id += 1;

        let req = RpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let mut line = serde_json::to_vec(&req).context("encoding sidecar request")?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("writing sidecar request")?;
        self.stdin.flush().await.context("flushing sidecar stdin")?;

        loop {
            let mut response = String::new();
            let bytes = self
                .stdout
                .read_line(&mut response)
                .await
                .context("reading sidecar response")?;
            if bytes == 0 {
                return Err(anyhow!("sidecar exited before replying"));
            }

            let value: Value = match serde_json::from_str(&response) {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        line = %response.trim_end(),
                        "ignoring non-JSON sidecar stdout line"
                    );
                    continue;
                }
            };
            if value
                .get("undercover_reverse_rpc")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let reverse: ReverseRpcRequest =
                    serde_json::from_value(value).context("decoding reverse RPC request")?;
                anyhow::ensure!(
                    reverse.undercover_reverse_rpc,
                    "invalid reverse RPC marker from sidecar"
                );
                let (tor, rpc_url) = reverse_rpc
                    .as_ref()
                    .ok_or_else(|| anyhow!("sidecar requested reverse RPC without a handler"))?;
                let result = if reverse.method == "__http_request" {
                    let request: rpc::ReverseHttpRequest =
                        serde_json::from_value(reverse.params).context("decoding reverse HTTP")?;
                    rpc::raw_http_request(tor.clone(), request)
                        .await
                        .and_then(|response| serde_json::to_value(response).map_err(Into::into))
                } else {
                    rpc::raw_request(
                        tor.clone(),
                        rpc_url.clone(),
                        &reverse.method,
                        reverse.params,
                    )
                    .await
                };
                let response = match result {
                    Ok(result) => ReverseRpcResponse {
                        undercover_reverse_rpc: true,
                        id: reverse.id,
                        result: Some(result),
                        error: None,
                    },
                    Err(err) => ReverseRpcResponse {
                        undercover_reverse_rpc: true,
                        id: reverse.id,
                        result: None,
                        error: Some(err.to_string()),
                    },
                };
                let mut line =
                    serde_json::to_vec(&response).context("encoding reverse RPC response")?;
                line.push(b'\n');
                self.stdin
                    .write_all(&line)
                    .await
                    .context("writing reverse RPC response")?;
                self.stdin
                    .flush()
                    .await
                    .context("flushing reverse RPC response")?;
                continue;
            }

            let response: RpcResponse<Res> =
                serde_json::from_value(value).context("decoding sidecar JSON-RPC response")?;
            anyhow::ensure!(
                response.id == id,
                "sidecar response id mismatch: expected {id}, got {}",
                response.id
            );

            if let Some(error) = response.error {
                if let Some(data) = error.data {
                    return Err(anyhow!(
                        "sidecar error {}: {}; data: {}",
                        error.code,
                        error.message,
                        data
                    ));
                }
                return Err(anyhow!("sidecar error {}: {}", error.code, error.message));
            }
            return response
                .result
                .ok_or_else(|| anyhow!("sidecar response had neither result nor error"));
        }
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        drop(self.stdin);
        let status = self
            .child
            .wait()
            .await
            .context("waiting for sidecar exit")?;
        anyhow::ensure!(status.success(), "sidecar exited with status {status}");
        Ok(())
    }
}

fn sidecar_command(workdir: &Path) -> anyhow::Result<Command> {
    let workdir = std::fs::canonicalize(workdir).context("resolving sidecar workdir")?;
    let mount_arg = format!(
        "type=bind,source={},target=/app/artifacts",
        workdir.join("artifacts").display()
    );
    let mut command = Command::new("docker");
    command.args([
        "run",
        "--rm",
        "-i",
        "--network",
        "none",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--env",
        "UNDERCOVER_FORBIDDEN_ENV=",
        "--mount",
        &mount_arg,
        "undercover-sidecar:dev",
    ]);
    Ok(command)
}
