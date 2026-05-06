//! Deno worker construction, V8 module-export lookup, and JSON-encoded
//! response decoding shared by the embedded runtime facade.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use deno_runtime::deno_core::{v8, Extension, FsModuleLoader, ModuleSpecifier};
use deno_runtime::deno_fs::RealFs;
use deno_runtime::deno_permissions::{
    Permissions, PermissionsContainer, PermissionsOptions, RuntimePermissionDescriptorParser,
};
use deno_runtime::deno_web::{BlobStore, InMemoryBroadcastChannel};
use deno_runtime::worker::{MainWorker, WorkerOptions, WorkerServiceOptions};
use deno_runtime::{deno_fetch, FeatureChecker};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::node_compat::{hermetic_node_state, NoNpm};

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

pub(super) fn create_worker(
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

pub(super) fn get_module_export(
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

pub(super) fn v8_to_string(
    worker: &mut MainWorker,
    value: v8::Global<v8::Value>,
) -> Result<String> {
    deno_runtime::deno_core::scope!(scope, worker.js_runtime);
    let local = v8::Local::new(scope, value);
    local
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
        .ok_or_else(|| anyhow!("result was not a string"))
}

pub(super) fn decode_call_response<Res>(response: &Value) -> Result<Res>
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
