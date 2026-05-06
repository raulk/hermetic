//! Host operations exposed to the embedded Railgun runtime.

use std::cell::RefCell;
use std::rc::Rc;

use deno_error::JsErrorBox;
use deno_runtime::deno_core::{extension, op2, JsBuffer, OpState, ToJsBuffer};

use crate::railgun::reverse::{ReverseRequest, ReverseResponse};
use crate::railgun::Artifact;
use crate::rpc::TorRpcClient;
use crate::tor::services;

pub struct EmbeddedHostState {
    artifact: Artifact,
    pub(super) rpc_client: Option<TorRpcClient>,
}

impl EmbeddedHostState {
    #[must_use]
    pub fn new(artifact: Artifact) -> Self {
        Self {
            artifact,
            rpc_client: None,
        }
    }
}

#[op2(fast)]
fn op_hermetic_log(#[string] message: &str) {
    tracing::debug!(target: "hermetic::runtime", "{message}");
}

#[op2(fast)]
fn op_hermetic_progress(#[string] message: &str) {
    tracing::info!(target: "hermetic::runtime", "{message}");
}

#[op2]
#[string]
fn op_hermetic_workdir(state: &mut OpState) -> String {
    state
        .borrow::<EmbeddedHostState>()
        .artifact
        .workdir()
        .to_string_lossy()
        .into_owned()
}

#[op2]
#[serde]
fn op_hermetic_read_artifact(
    state: &mut OpState,
    #[string] relative_path: &str,
) -> Result<ToJsBuffer, JsErrorBox> {
    state
        .borrow::<EmbeddedHostState>()
        .artifact
        .read(relative_path)
        .map(ToJsBuffer::from)
        .map_err(|err| JsErrorBox::generic(err.to_string()))
}

#[op2]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 buffer arguments are owned"
)]
fn op_hermetic_write_artifact(
    state: &mut OpState,
    #[string] dir: &str,
    #[string] relative_path: &str,
    #[buffer] bytes: JsBuffer,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EmbeddedHostState>()
        .artifact
        .write(dir, relative_path, bytes.as_ref())
        .map_err(|err| JsErrorBox::generic(err.to_string()))
}

#[op2(fast)]
fn op_hermetic_artifact_exists(state: &mut OpState, #[string] relative_path: &str) -> bool {
    state
        .borrow::<EmbeddedHostState>()
        .artifact
        .exists(relative_path)
}

#[op2]
#[string]
fn op_hermetic_service_endpoint(#[string] service: &str) -> Result<String, JsErrorBox> {
    services::service_endpoint(service)
        .map(str::to_owned)
        .map_err(|err| JsErrorBox::generic(err.to_string()))
}

#[op2]
#[serde]
async fn op_hermetic_reverse_request(
    state: Rc<RefCell<OpState>>,
    #[serde] request: ReverseRequest,
) -> Result<ReverseResponse, JsErrorBox> {
    let rpc_client = {
        let state = state.borrow();
        state
            .borrow::<EmbeddedHostState>()
            .rpc_client
            .clone()
            .ok_or_else(|| JsErrorBox::generic("reverse request attempted without RPC client"))?
    };
    rpc_client
        .handle_reverse_request(request)
        .await
        .map_err(|err| JsErrorBox::generic(format!("{err:#}")))
}

extension!(
    hermetic_host_ops,
    ops = [
        op_hermetic_log,
        op_hermetic_progress,
        op_hermetic_read_artifact,
        op_hermetic_write_artifact,
        op_hermetic_artifact_exists,
        op_hermetic_service_endpoint,
        op_hermetic_reverse_request,
        op_hermetic_workdir,
    ],
    esm_entry_point = "ext:hermetic_host_ops/bootstrap.js",
    esm = [dir "src/embedded", "bootstrap.js"],
    options = {
        host_state: EmbeddedHostState,
    },
    state = |state, options| {
        state.put(options.host_state);
    }
);
