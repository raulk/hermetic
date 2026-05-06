#[cfg(feature = "deno-embed")]
use deno_core::{extension, op2, JsRuntime, RuntimeOptions};

#[cfg(feature = "deno-embed")]
#[op2]
#[string]
fn op_hermetic_rpc(#[string] request: String) -> Result<String, deno_error::JsErrorBox> {
    let request: serde_json::Value = serde_json::from_str(&request)
        .map_err(|err| deno_error::JsErrorBox::generic(err.to_string()))?;
    Ok(serde_json::json!({
        "ok": true,
        "transport": "rust-op",
        "rpc_method": request.get("method").and_then(|value| value.as_str()),
    })
    .to_string())
}

#[cfg(feature = "deno-embed")]
extension!(
    hermetic_embed,
    ops = [op_hermetic_rpc],
    esm_entry_point = "ext:hermetic_embed/deno_embed_bootstrap.js",
    esm = [ dir "examples", "deno_embed_bootstrap.js" ],
);

#[cfg(feature = "deno-embed")]
fn main() -> anyhow::Result<()> {
    let mut runtime = JsRuntime::new(RuntimeOptions {
        extensions: vec![hermetic_embed::init()],
        ..Default::default()
    });

    runtime.execute_script(
        "hermetic:deno_embed",
        r#"
const rpc = globalThis.hermetic.rpc({ method: "eth_chainId", params: [] });
if (!rpc.ok || rpc.transport !== "rust-op" || rpc.rpc_method !== "eth_chainId") {
  throw new Error(`unexpected rpc response: ${JSON.stringify(rpc)}`);
}
if (typeof globalThis.fetch !== "undefined") {
  throw new Error("deno_core unexpectedly exposes ambient fetch");
}
if (typeof globalThis.process !== "undefined") {
  throw new Error("deno_core unexpectedly exposes node process");
}
globalThis.__hermetic_result = {
  rpc,
  fetch: typeof globalThis.fetch,
  process: typeof globalThis.process,
};
"#,
    )?;

    let result = runtime.execute_script(
        "hermetic:deno_embed_result",
        "JSON.stringify(globalThis.__hermetic_result)",
    )?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, result);
    let result = local
        .to_string(scope)
        .ok_or_else(|| anyhow::anyhow!("result was not a string"))?
        .to_rust_string_lossy(scope);
    println!("{result}");
    Ok(())
}

#[cfg(not(feature = "deno-embed"))]
fn main() {
    eprintln!("re-run with --features deno-embed");
    std::process::exit(2);
}
