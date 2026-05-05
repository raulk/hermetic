use std::path::Path;

use anyhow::Context as _;
use arti_client::{config::TorClientConfigBuilder, TorClient};
use tor_rtcompat::PreferredRuntime;

pub type ArtiClient = TorClient<PreferredRuntime>;

#[derive(Clone, Copy, Debug)]
pub enum IsolationLabel {
    EventSync,
    Shield,
    Transfer,
    Unshield,
}

pub async fn bootstrap(state_dir: &Path, cache_dir: &Path) -> anyhow::Result<ArtiClient> {
    let config = TorClientConfigBuilder::from_directories(state_dir, cache_dir)
        .build()
        .context("building Tor client config via Arti")?;

    TorClient::create_bootstrapped(config)
        .await
        .context("bootstrapping Tor client via Arti")
}

pub fn isolated_for(client: &ArtiClient, _label: IsolationLabel) -> ArtiClient {
    client.isolated_client()
}
