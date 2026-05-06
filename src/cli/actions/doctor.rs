use anyhow::{Context as _, Result};

use crate::cli::args::WorkdirArgs;
use crate::railgun::RailgunRuntime;

pub(crate) async fn run(workdir: WorkdirArgs) -> Result<()> {
    let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("binding local probe socket")?;
    let node_net_port = listener.local_addr().context("local listener addr")?.port();
    let health = runtime.health().await?;
    println!("sdk_version={}", health.sdk_version);
    println!("shared_models_version={}", health.shared_models_version);
    println!("node_compat={}", health.node_compat);
    anyhow::ensure!(health.node_compat, "embedded SDK imports did not load");

    let smoke = runtime.check_perms(node_net_port).await?;
    println!("fetch_denied={}", smoke.fetch_denied);
    println!("connect_denied={}", smoke.connect_denied);
    println!("node_net_denied={}", smoke.node_net_denied);
    println!("write_denied={}", smoke.write_denied);
    println!("env_denied={}", smoke.env_denied);
    println!("read_allowed={}", smoke.read_allowed);
    anyhow::ensure!(smoke.fetch_denied, "embedded Deno fetch was not denied");
    anyhow::ensure!(smoke.connect_denied, "embedded Deno connect was not denied");
    anyhow::ensure!(smoke.node_net_denied, "embedded node:net was not denied");
    anyhow::ensure!(
        smoke.write_denied,
        "embedded write outside artifacts was not denied"
    );
    anyhow::ensure!(smoke.env_denied, "embedded broad env read was not denied");
    anyhow::ensure!(smoke.read_allowed, "embedded artifact read was denied");
    Ok(())
}
