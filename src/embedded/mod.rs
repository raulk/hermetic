//! Embedded Deno worker that hosts the bundled Railgun SDK runtime, plus
//! the host op surface and the reverse-RPC plumbing.

mod node_compat;
mod ops;
mod worker;

use anyhow::{anyhow, Context as _, Result};
use deno_runtime::deno_core::{serde_v8, v8, ModuleSpecifier, PollEventLoopOptions};
use deno_runtime::deno_permissions::PermissionsContainer;
use deno_runtime::worker::MainWorker;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::railgun::reverse::ReverseRpcService;

pub use ops::EmbeddedHostState;
pub use worker::permissions_from_options;

pub struct EmbeddedDeno {
    worker: MainWorker,
    invoke: v8::Global<v8::Function>,
}

impl EmbeddedDeno {
    /// Create an embedded Deno worker and load the bundled runtime as ESM.
    ///
    /// # Errors
    ///
    /// Returns an error if Deno worker initialization, module loading, module
    /// evaluation, or export lookup fails.
    pub async fn load_esm(
        main_module: &ModuleSpecifier,
        source: String,
        permissions: PermissionsContainer,
        host_state: EmbeddedHostState,
    ) -> Result<Self> {
        let mut worker = worker::create_worker(
            main_module,
            permissions,
            vec![ops::hermetic_host_ops::init(host_state)],
        );
        let module_id = worker
            .js_runtime
            .load_main_es_module_from_code(main_module, source)
            .await
            .context("loading embedded ESM module")?;
        worker
            .evaluate_module(module_id)
            .await
            .context("evaluating embedded ESM module")?;
        let invoke = worker::get_module_export(&mut worker, module_id, "invoke")?;
        Ok(Self { worker, invoke })
    }

    /// Call a Railgun runtime method without reverse RPC.
    ///
    /// # Errors
    ///
    /// Returns an error if the JavaScript call fails or its result cannot be
    /// deserialized into `Res`.
    pub async fn call<Req, Res>(&mut self, method: &str, params: Req) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.call_inner(method, params, None).await
    }

    /// Call a Railgun runtime method while servicing reverse JSON-RPC/HTTP via Tor.
    ///
    /// # Errors
    ///
    /// Returns an error if JavaScript execution fails, reverse RPC fails, or the
    /// final result cannot be deserialized into `Res`.
    pub async fn call_with_reverse<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        reverse: ReverseRpcService,
    ) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.call_inner(method, params, Some(reverse)).await
    }

    async fn call_inner<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        reverse: Option<ReverseRpcService>,
    ) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.set_reverse(reverse);
        let call_result = self.call_runtime(method, params).await;
        self.set_reverse(None);
        worker::decode_call_response(&call_result?)
    }

    async fn call_runtime<Req>(&mut self, method: &str, params: Req) -> Result<Value>
    where
        Req: Serialize,
    {
        let args = self.encode_call_args(method, params)?;
        let call = self.worker.js_runtime.call_with_args(&self.invoke, &args);
        let value = self
            .worker
            .js_runtime
            .with_event_loop_promise(call, PollEventLoopOptions::default())
            .await?;
        let json = worker::v8_to_string(&mut self.worker, value)?;
        serde_json::from_str(&json).map_err(Into::into)
    }

    fn encode_call_args<Req>(
        &mut self,
        method: &str,
        params: Req,
    ) -> Result<[v8::Global<v8::Value>; 2]>
    where
        Req: Serialize,
    {
        deno_runtime::deno_core::scope!(scope, self.worker.js_runtime);
        let method = serde_v8::to_v8(scope, method)
            .map_err(|err| anyhow!("encoding embedded method argument: {err}"))?;
        let params = serde_v8::to_v8(scope, params)
            .map_err(|err| anyhow!("encoding embedded params argument: {err}"))?;
        Ok([
            v8::Global::new(scope, method),
            v8::Global::new(scope, params),
        ])
    }

    fn set_reverse(&mut self, reverse: Option<ReverseRpcService>) {
        let op_state = self.worker.js_runtime.op_state();
        let mut state = op_state.borrow_mut();
        state.borrow_mut::<EmbeddedHostState>().reverse = reverse;
    }
}
