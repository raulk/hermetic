#[cfg(feature = "deno-runtime")]
use std::{borrow::Cow, path::Path, path::PathBuf, rc::Rc, sync::Arc};

use deno_error::JsErrorBox;
#[cfg(feature = "deno-runtime")]
use deno_runtime::{
    deno_core::{extension, resolve_url, FastString, FsModuleLoader, ModuleSpecifier},
    deno_fetch,
    deno_fs::RealFs,
    deno_node::{NodeRequireLoader, NodeRequireLoaderRc},
    deno_permissions::{PermissionsContainer, RuntimePermissionDescriptorParser},
    deno_web::{BlobStore, InMemoryBroadcastChannel},
    worker::{MainWorker, WorkerOptions, WorkerServiceOptions},
    FeatureChecker,
};

#[cfg(feature = "deno-runtime")]
#[derive(Debug)]
struct LocalNodeRequireLoader;

#[cfg(feature = "deno-runtime")]
impl NodeRequireLoader for LocalNodeRequireLoader {
    fn ensure_read_permission<'a>(
        &self,
        _permissions: &mut PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, JsErrorBox> {
        Ok(path)
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

#[cfg(feature = "deno-runtime")]
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
        state.put(sys_traits::impls::RealSys);
        state.put::<NodeRequireLoaderRc>(Rc::new(LocalNodeRequireLoader));
        state.put(pkg_json_resolver);
        state.put(node_resolver);
    }
);

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
    let mut options = WorkerOptions::default();
    options.extensions.push(hermetic_node_state::init());
    MainWorker::bootstrap_from_options(main_module, services, options)
}

#[cfg(feature = "deno-runtime")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let main_module = resolve_url("file:///hermetic-runtime-railgun.js")?;
    let mut worker = worker(&main_module);
    worker.execute_script(
        "hermetic:railgun-require",
        r#"
globalThis.__hermetic_ready = import("node:module").then((module) => {
  globalThis.require = module.createRequire("file://" + Deno.cwd() + "/sidecar/runtime.mjs");
});
"#
        .to_string()
        .into(),
    )?;
    worker.run_event_loop(false).await?;
    worker.execute_script(
        "hermetic:railgun-require-check",
        r#"
if (typeof require !== "function") {
  throw new Error(`node require unavailable: ${typeof require}`);
}
"#
        .to_string()
        .into(),
    )?;

    let bundle = std::fs::read_to_string("embedded/railgun_runtime.iife.js")?;
    worker.execute_script("hermetic:railgun-bundle", bundle.into())?;
    worker.execute_script(
        "hermetic:railgun-health",
        r#"
globalThis.__hermetic_ready = HermeticRailgunRuntime.handle("health").then((result) => {
  globalThis.__hermetic_result = result;
});
"#
        .to_string()
        .into(),
    )?;
    worker.run_event_loop(false).await?;
    let result = worker.execute_script(
        "hermetic:railgun-health-result",
        "JSON.stringify(globalThis.__hermetic_result)"
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
