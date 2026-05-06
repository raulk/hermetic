use std::path::Path;

use anyhow::{Context as _, Result};
use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use tor_rtcompat::PreferredRuntime;

pub type ArtiClient = TorClient<PreferredRuntime>;

/// Bootstrap a Tor client using persistent state and cache directories.
///
/// # Errors
///
/// Returns an error if the Arti configuration cannot be built or Tor bootstrap
/// fails.
pub async fn bootstrap(state_dir: &Path, cache_dir: &Path) -> Result<ArtiClient> {
    let config = TorClientConfigBuilder::from_directories(state_dir, cache_dir)
        .build()
        .context("building Tor client config via Arti")?;

    TorClient::create_bootstrapped(config)
        .await
        .context("bootstrapping Tor client via Arti")
}

pub fn isolated_client(client: &ArtiClient) -> ArtiClient {
    client.isolated_client()
}
