use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use deno_error::JsErrorBox;
use deno_runtime::deno_core::{
    extension, op2, serde_v8, v8, Extension, FastString, FsModuleLoader, JsBuffer, ModuleSpecifier,
    OpState, PollEventLoopOptions, ToJsBuffer,
};
use deno_runtime::deno_fs::RealFs;
use deno_runtime::deno_node::{NodeRequireLoader, NodeRequireLoaderRc};
use deno_runtime::deno_permissions::{
    OpenAccessKind, Permissions, PermissionsContainer, PermissionsOptions,
    RuntimePermissionDescriptorParser,
};
use deno_runtime::deno_web::{BlobStore, InMemoryBroadcastChannel};
use deno_runtime::worker::{MainWorker, WorkerOptions, WorkerServiceOptions};
use deno_runtime::{deno_fetch, FeatureChecker};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::railgun::reverse::{self, ReverseRequest, ReverseResponse};
use crate::railgun::Artifact;
use crate::rpc::TorRpcClient;

#[derive(Debug)]
struct NoNpm;

impl node_resolver::InNpmPackageChecker for NoNpm {
    fn in_npm_package(&self, _specifier: &deno_runtime::deno_core::url::Url) -> bool {
        false
    }
}

impl node_resolver::NpmPackageFolderResolver for NoNpm {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &node_resolver::UrlOrPathRef,
    ) -> Result<PathBuf, node_resolver::errors::PackageFolderResolveError> {
        Err(node_resolver::errors::PackageFolderResolveError(Box::new(
            node_resolver::errors::PackageFolderResolveErrorKind::PackageNotFound(
                node_resolver::errors::PackageNotFoundError {
                    package_name: specifier.to_string(),
                    referrer: referrer.display(),
                    referrer_extra: None,
                },
            ),
        )))
    }

    fn resolve_types_package_folder(
        &self,
        _types_package_name: &str,
        _maybe_package_version: Option<&deno_semver::Version>,
        _maybe_referrer: Option<&node_resolver::UrlOrPathRef>,
    ) -> Option<PathBuf> {
        None
    }
}

#[derive(Debug)]
struct LocalNodeRequireLoader;

impl NodeRequireLoader for LocalNodeRequireLoader {
    fn ensure_read_permission<'a>(
        &self,
        permissions: &mut PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, JsErrorBox> {
        permissions
            .check_open(path, OpenAccessKind::Read, Some("node:require"))
            .map(deno_runtime::deno_permissions::CheckedPath::into_path)
            .map_err(JsErrorBox::from_err)
    }

    fn load_text_file_lossy(&self, path: &Path) -> Result<FastString, JsErrorBox> {
        std::fs::read_to_string(path)
            .map(Into::into)
            .map_err(|err| JsErrorBox::generic(err.to_string()))
    }

    fn is_maybe_cjs(
        &self,
        _specifier: &deno_runtime::deno_core::url::Url,
    ) -> Result<bool, node_resolver::errors::PackageJsonLoadError> {
        Ok(true)
    }
}

extension!(
    hermetic_node_state,
    state = |state| {
        let sys = sys_traits::impls::RealSys;
        let pkg_json_resolver =
            Arc::new(node_resolver::PackageJsonResolver::new(sys.clone(), None));
        let node_resolver = Arc::new(node_resolver::NodeResolver::new(
            NoNpm,
            node_resolver::DenoIsBuiltInNodeModuleChecker,
            NoNpm,
            pkg_json_resolver.clone(),
            node_resolver::cache::NodeResolutionSys::new(sys.clone(), None),
            node_resolver::NodeResolverOptions {
                conditions: node_resolver::NodeConditionOptions::default(),
                is_browser_platform: false,
                bundle_mode: false,
                typescript_version: None,
            },
        ));
        state.put(sys);
        state.put::<NodeRequireLoaderRc>(Rc::new(LocalNodeRequireLoader));
        state.put(pkg_json_resolver);
        state.put(node_resolver);
    }
);

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
    esm_entry_point = "ext:hermetic_host_ops/hermetic_host_ops.js",
    esm = [dir "src", "hermetic_host_ops.js"],
    options = {
        host_state: EmbeddedHostState,
    },
    state = |state, options| {
        state.put(options.host_state);
    }
);

pub struct EmbeddedHostState {
    artifact: Artifact,
    rpc_client: Option<TorRpcClient>,
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
    reverse::service_endpoint(service)
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
        let mut worker = create_worker(
            main_module,
            permissions,
            vec![hermetic_host_ops::init(host_state)],
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
        let invoke = get_module_export(&mut worker, module_id, "invoke")?;
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
    pub async fn call_with_reverse_rpc<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        rpc_client: TorRpcClient,
    ) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.call_inner(method, params, Some(rpc_client)).await
    }

    async fn call_inner<Req, Res>(
        &mut self,
        method: &str,
        params: Req,
        rpc_client: Option<TorRpcClient>,
    ) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        self.set_rpc_client(rpc_client);
        let call_result = self.call_runtime(method, params).await;
        self.set_rpc_client(None);
        decode_call_response(&call_result?)
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
        let json = v8_to_string(&mut self.worker, value)?;
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

    fn set_rpc_client(&mut self, rpc_client: Option<TorRpcClient>) {
        let op_state = self.worker.js_runtime.op_state();
        let mut state = op_state.borrow_mut();
        state.borrow_mut::<EmbeddedHostState>().rpc_client = rpc_client;
    }
}

fn decode_call_response<Res>(response: &Value) -> Result<Res>
where
    Res: DeserializeOwned,
{
    if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        serde_json::from_value(response["result"].clone()).map_err(Into::into)
    } else {
        Err(anyhow!(
            "embedded Railgun error: {}",
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ))
    }
}

/// Build a Deno permissions container from explicit permission options.
///
/// # Errors
///
/// Returns an error if Deno rejects the permission configuration.
pub fn permissions_from_options(options: &PermissionsOptions) -> Result<PermissionsContainer> {
    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        sys_traits::impls::RealSys,
    ));
    let permissions = Permissions::from_options(parser.as_ref(), options)?;
    Ok(PermissionsContainer::new(parser, permissions))
}

fn create_worker(
    main_module: &ModuleSpecifier,
    permissions: PermissionsContainer,
    extensions: Vec<Extension>,
) -> MainWorker {
    let services = WorkerServiceOptions::<NoNpm, NoNpm, sys_traits::impls::RealSys> {
        blob_store: Arc::new(BlobStore::default()),
        broadcast_channel: InMemoryBroadcastChannel::default(),
        deno_rt_native_addon_loader: None,
        feature_checker: Arc::new(FeatureChecker::default()),
        fs: Arc::new(RealFs),
        module_loader: Rc::new(FsModuleLoader),
        node_services: None,
        npm_process_state_provider: None,
        permissions,
        root_cert_store_provider: None,
        fetch_dns_resolver: deno_fetch::dns::Resolver::default(),
        shared_array_buffer_store: None,
        compiled_wasm_module_store: None,
        v8_code_cache: None,
        bundle_provider: None,
    };
    let mut options = WorkerOptions::default();
    options.extensions.extend(extensions);
    options.extensions.push(hermetic_node_state::init());
    MainWorker::bootstrap_from_options(main_module, services, options)
}

fn get_module_export(
    worker: &mut MainWorker,
    module_id: deno_runtime::deno_core::ModuleId,
    export_name: &str,
) -> Result<v8::Global<v8::Function>> {
    let namespace = worker
        .js_runtime
        .get_module_namespace(module_id)
        .context("getting embedded module namespace")?;
    deno_runtime::deno_core::scope!(scope, worker.js_runtime);
    let namespace = v8::Local::new(scope, namespace);
    let function_key =
        v8::String::new(scope, export_name).ok_or_else(|| anyhow!("allocating V8 string"))?;
    let value = namespace
        .get(scope, function_key.into())
        .ok_or_else(|| anyhow!("module export {export_name} is missing"))?;
    let function = v8::Local::<v8::Function>::try_from(value)
        .map_err(|_| anyhow!("module export {export_name} is not a function"))?;
    Ok(v8::Global::new(scope, function))
}

fn v8_to_string(worker: &mut MainWorker, value: v8::Global<v8::Value>) -> Result<String> {
    deno_runtime::deno_core::scope!(scope, worker.js_runtime);
    let local = v8::Local::new(scope, value);
    local
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
        .ok_or_else(|| anyhow!("result was not a string"))
}
