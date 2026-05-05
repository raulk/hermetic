use std::{borrow::Cow, path::Path, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use anyhow::{anyhow, Context as _, Result};
use deno_error::JsErrorBox;
use deno_runtime::{
    deno_core::{extension, resolve_url, v8, FastString, FsModuleLoader, ModuleSpecifier},
    deno_fetch,
    deno_fs::RealFs,
    deno_node::{NodeRequireLoader, NodeRequireLoaderRc},
    deno_permissions::{
        OpenAccessKind, Permissions, PermissionsContainer, PermissionsOptions,
        RuntimePermissionDescriptorParser,
    },
    deno_web::{BlobStore, InMemoryBroadcastChannel},
    worker::{MainWorker, WorkerOptions, WorkerServiceOptions},
    FeatureChecker,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::rpc::{ReverseEnvelope, TorRpcClient};

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
    undercover_node_state,
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

pub struct EmbeddedRailgun {
    worker: MainWorker,
    next_call_id: u64,
}

impl EmbeddedRailgun {
    /// Create an embedded Deno worker and load the generated Railgun bundle.
    ///
    /// # Errors
    ///
    /// Returns an error if the workdir cannot be resolved, the generated bundle
    /// cannot be read, or Deno worker initialization fails.
    pub async fn new(workdir: &Path) -> Result<Self> {
        let workdir = std::fs::canonicalize(workdir).context("resolving embedded workdir")?;
        let main_module = resolve_url("file:///undercover-embedded-railgun.js")?;
        let mut worker = create_worker(&main_module, &workdir)?;
        let bundle_path = workdir.join("embedded/railgun_runtime.iife.js");
        let bundle = std::fs::read_to_string(&bundle_path)
            .with_context(|| format!("reading {}", bundle_path.display()))?;
        let cwd = serde_json::to_string(&workdir.to_string_lossy())?;
        worker.execute_script(
            "undercover:embedded-bootstrap",
            format!(
                r#"
globalThis.__undercover_workdir = {cwd};
globalThis.__undercover_reverse = [];
globalThis.__undercover_deno_fetch = globalThis.fetch;
globalThis.__undercover_require_ready = import("node:module").then((module) => {{
  globalThis.require = module.createRequire("file://" + globalThis.__undercover_workdir + "/railgun-runtime/runtime.mjs");
}});
async function __undercover_denied(op) {{
  try {{
    await op();
    return false;
  }} catch (_) {{
    return true;
  }}
}}
globalThis.__undercover_host = {{
  writeLine(line) {{ globalThis.__undercover_reverse.push(JSON.parse(line)); }},
  log(message) {{ console.error(message); }},
  readArtifact(relativePath) {{
    try {{
      const path = `${{globalThis.__undercover_workdir}}/artifacts/${{relativePath}}`;
      return Deno.readFileSync(path);
    }} catch (_) {{
      return null;
    }}
  }},
  writeArtifact(dir, relativePath, item) {{
    Deno.mkdirSync(`${{globalThis.__undercover_workdir}}/artifacts/${{dir}}`, {{ recursive: true }});
    Deno.writeFileSync(`${{globalThis.__undercover_workdir}}/artifacts/${{relativePath}}`, item);
  }},
  artifactExists(relativePath) {{
    try {{
      Deno.statSync(`${{globalThis.__undercover_workdir}}/artifacts/${{relativePath}}`);
      return true;
    }} catch (_) {{
      return false;
    }}
  }},
  async permissionSmoke(params = {{}}) {{
    const net = require("node:net");
    const nodeNetHost = params.node_net_host ?? "127.0.0.1";
    const nodeNetPort = params.node_net_port ?? 53;
    return {{
      fetch_denied: await __undercover_denied(() => globalThis.__undercover_deno_fetch("https://example.com")),
      connect_denied: await __undercover_denied(() => Deno.connect({{ hostname: "1.1.1.1", port: 53 }})),
      node_net_denied: await __undercover_denied(() => new Promise((resolve, reject) => {{
        const socket = net.connect(nodeNetPort, nodeNetHost);
        socket.once("connect", () => {{ socket.destroy(); resolve(); }});
        socket.once("error", reject);
        socket.setTimeout(1000, () => {{ socket.destroy(); reject(new Error("socket timeout")); }});
      }})),
      write_denied: await __undercover_denied(() => Deno.writeTextFile("/tmp/undercover-deny-write", "x")),
      env_denied: await __undercover_denied(() => Deno.env.get("UNDERCOVER_FORBIDDEN_ENV")),
      read_allowed: !await __undercover_denied(() => Deno.readTextFile(`${{globalThis.__undercover_workdir}}/artifacts/manifest`)),
    }};
  }},
}};
"#,
            )
            .into(),
        )?;
        worker.run_event_loop(false).await?;
        worker.execute_script("undercover:embedded-bundle", bundle.into())?;
        Ok(Self {
            worker,
            next_call_id: 1,
        })
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
        let id = self.next_call_id;
        self.next_call_id += 1;
        let method = serde_json::to_string(method)?;
        let params = serde_json::to_string(&params)?;
        self.worker.execute_script(
            "undercover:embedded-call",
            format!(
                r"
globalThis.__undercover_call_{id} = UndercoverRailgunRuntime.handle({method}, {params}).then(
  (result) => {{ globalThis.__undercover_result_{id} = {{ ok: true, result }}; }},
  (error) => {{ globalThis.__undercover_result_{id} = {{ ok: false, error: String(error?.stack ?? error) }}; }},
);
"
            )
            .into(),
        )?;
        self.worker.run_event_loop(false).await?;
        let value = self.worker.execute_script(
            "undercover:embedded-call-result",
            format!("JSON.stringify(globalThis.__undercover_result_{id})").into(),
        )?;
        let json = v8_to_string(&mut self.worker, value)?;
        let response: Value = serde_json::from_str(&json)?;
        if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            serde_json::from_value(response["result"].clone()).map_err(Into::into)
        } else {
            anyhow::bail!(
                "embedded Railgun error: {}",
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            )
        }
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
        let id = self.next_call_id;
        self.next_call_id += 1;
        let method_json = serde_json::to_string(method)?;
        let params_json = serde_json::to_string(&params)?;
        self.worker.execute_script(
            "undercover:embedded-call-rpc",
            format!(
                r"
globalThis.__undercover_result_{id} = undefined;
globalThis.__undercover_call_{id} = UndercoverRailgunRuntime.handle({method_json}, {params_json}).then(
  (result) => {{ globalThis.__undercover_result_{id} = {{ ok: true, result }}; }},
  (error) => {{ globalThis.__undercover_result_{id} = {{ ok: false, error: String(error?.stack ?? error) }}; }},
);
"
            )
            .into(),
        )?;

        loop {
            self.worker
                .run_up_to_duration(Duration::from_millis(10))
                .await?;

            if let Some(response) = self.take_call_result(id)? {
                return decode_call_response(&response);
            }

            while let Some(reverse) = self.take_reverse_request()? {
                let (reverse_id, reverse) = reverse.into_request()?;
                let result = rpc_client.handle_reverse_request(reverse).await;
                let response = match result {
                    Ok(result) => serde_json::json!({
                        "undercover_reverse_rpc": true,
                        "id": reverse_id,
                        "result": result,
                    }),
                    Err(err) => serde_json::json!({
                        "undercover_reverse_rpc": true,
                        "id": reverse_id,
                        "error": err.to_string(),
                    }),
                };
                self.worker.execute_script(
                    "undercover:embedded-reverse-response",
                    format!(
                        "UndercoverRailgunRuntime.handleReverseRpcResponse({});",
                        serde_json::to_string(&response)?
                    )
                    .into(),
                )?;
            }
        }
    }

    fn take_call_result(&mut self, id: u64) -> Result<Option<Value>> {
        let value = self.worker.execute_script(
            "undercover:embedded-result-poll",
            format!(
                r#"
(() => {{
  const result = globalThis.__undercover_result_{id};
  if (result === undefined) return "null";
  delete globalThis.__undercover_result_{id};
  return JSON.stringify(result);
}})()
"#
            )
            .into(),
        )?;
        let json = v8_to_string(&mut self.worker, value)?;
        serde_json::from_str(&json).map_err(Into::into)
    }

    fn take_reverse_request(&mut self) -> Result<Option<ReverseEnvelope>> {
        let value = self.worker.execute_script(
            "undercover:embedded-reverse-poll",
            r"JSON.stringify(globalThis.__undercover_reverse.shift() ?? null)"
                .to_string()
                .into(),
        )?;
        let json = v8_to_string(&mut self.worker, value)?;
        serde_json::from_str(&json).map_err(Into::into)
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

fn create_worker(main_module: &ModuleSpecifier, workdir: &Path) -> Result<MainWorker> {
    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        sys_traits::impls::RealSys,
    ));
    let artifacts = workdir.join("artifacts").to_string_lossy().to_string();
    let wasm_packages = workdir
        .join("railgun-runtime/node_modules/@railgun-community")
        .to_string_lossy()
        .to_string();
    let permissions = Permissions::from_options(
        parser.as_ref(),
        &PermissionsOptions {
            allow_read: Some(vec![artifacts.clone(), wasm_packages]),
            allow_write: Some(vec![artifacts]),
            allow_env: Some(vec![
                "WS_NO_BUFFER_UTIL".to_string(),
                "WS_NO_UTF_8_VALIDATE".to_string(),
                "READABLE_STREAM".to_string(),
                "NODE_ENV".to_string(),
            ]),
            prompt: false,
            ..Default::default()
        },
    )?;
    let services = WorkerServiceOptions::<NoNpm, NoNpm, sys_traits::impls::RealSys> {
        blob_store: Arc::new(BlobStore::default()),
        broadcast_channel: InMemoryBroadcastChannel::default(),
        deno_rt_native_addon_loader: None,
        feature_checker: Arc::new(FeatureChecker::default()),
        fs: Arc::new(RealFs),
        module_loader: Rc::new(FsModuleLoader),
        node_services: None,
        npm_process_state_provider: None,
        permissions: PermissionsContainer::new(parser, permissions),
        root_cert_store_provider: None,
        fetch_dns_resolver: deno_fetch::dns::Resolver::default(),
        shared_array_buffer_store: None,
        compiled_wasm_module_store: None,
        v8_code_cache: None,
        bundle_provider: None,
    };
    let mut options = WorkerOptions::default();
    options.extensions.push(undercover_node_state::init());
    Ok(MainWorker::bootstrap_from_options(
        main_module,
        services,
        options,
    ))
}

fn v8_to_string(worker: &mut MainWorker, value: v8::Global<v8::Value>) -> Result<String> {
    deno_runtime::deno_core::scope!(scope, worker.js_runtime);
    let local = v8::Local::new(scope, value);
    local
        .to_string(scope)
        .map(|value| value.to_rust_string_lossy(scope))
        .ok_or_else(|| anyhow!("result was not a string"))
}
