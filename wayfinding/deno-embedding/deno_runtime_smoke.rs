#[cfg(feature = "deno-runtime")]
use std::{path::PathBuf, rc::Rc, sync::Arc};

#[cfg(feature = "deno-runtime")]
use deno_runtime::{
    deno_core::{resolve_url, FsModuleLoader, ModuleSpecifier},
    deno_fetch,
    deno_fs::RealFs,
    deno_permissions::{PermissionsContainer, RuntimePermissionDescriptorParser},
    deno_web::{BlobStore, InMemoryBroadcastChannel},
    worker::{MainWorker, WorkerOptions, WorkerServiceOptions},
    FeatureChecker,
};

#[cfg(feature = "deno-runtime")]
#[derive(Debug)]
struct NoNpm;

#[cfg(feature = "deno-runtime")]
impl node_resolver::InNpmPackageChecker for NoNpm {
    fn in_npm_package(&self, _specifier: &deno_runtime::deno_core::url::Url) -> bool {
        false
    }
}

#[cfg(feature = "deno-runtime")]
impl node_resolver::NpmPackageFolderResolver for NoNpm {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        _referrer: &node_resolver::UrlOrPathRef,
    ) -> Result<PathBuf, node_resolver::errors::PackageFolderResolveError> {
        Err(node_resolver::errors::PackageFolderResolveError(Box::new(
            node_resolver::errors::PackageFolderResolveErrorKind::PackageNotFound(
                node_resolver::errors::PackageNotFoundError {
                    package_name: specifier.to_string(),
                    referrer: _referrer.display(),
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

#[cfg(feature = "deno-runtime")]
fn worker(main_module: &ModuleSpecifier) -> MainWorker {
    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        sys_traits::impls::RealSys,
    ));
    let services = WorkerServiceOptions::<NoNpm, NoNpm, sys_traits::impls::RealSys> {
        blob_store: Arc::new(BlobStore::default()),
        broadcast_channel: InMemoryBroadcastChannel::default(),
        deno_rt_native_addon_loader: None,
        feature_checker: Arc::new(FeatureChecker::default()),
        fs: Arc::new(RealFs),
        module_loader: Rc::new(FsModuleLoader),
        node_services: None,
        npm_process_state_provider: None,
        permissions: PermissionsContainer::allow_all(parser),
        root_cert_store_provider: None,
        fetch_dns_resolver: deno_fetch::dns::Resolver::default(),
        shared_array_buffer_store: None,
        compiled_wasm_module_store: None,
        v8_code_cache: None,
        bundle_provider: None,
    };
    MainWorker::bootstrap_from_options(main_module, services, WorkerOptions::default())
}

#[cfg(feature = "deno-runtime")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let main_module = resolve_url("file:///undercover-runtime-smoke.js")?;
    let mut worker = worker(&main_module);
    worker.execute_script(
        "undercover:runtime-smoke",
        r#"
globalThis.__undercover_result = {
  deno: typeof Deno,
  fetch: typeof fetch,
  process: typeof process,
  require: typeof require,
};
"#
        .to_string()
        .into(),
    )?;
    worker.run_event_loop(false).await?;
    let result = worker.execute_script(
        "undercover:runtime-smoke-result",
        "JSON.stringify(globalThis.__undercover_result)"
            .to_string()
            .into(),
    )?;
    deno_runtime::deno_core::scope!(scope, worker.js_runtime);
    let local = deno_runtime::deno_core::v8::Local::new(scope, result);
    let result = local
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
        .ok_or_else(|| anyhow::anyhow!("result was not a string"))?;
    println!("{result}");
    Ok(())
}

#[cfg(not(feature = "deno-runtime"))]
fn main() {
    eprintln!("re-run with --features deno-runtime");
    std::process::exit(2);
}
