use anyhow::Result;
use clap::Parser;
use hermetic::cli::{run, Cli};

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hermetic=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Load .env from the current directory if present. Existing env vars
    // take precedence over file values; a missing .env is not an error.
    let _ = dotenvy::dotenv();
    init_tracing();
    // Installing the global crypto provider can legitimately fail after another
    // test or embedding path installed it first.
    let _ = rustls::crypto::ring::default_provider().install_default();
    run(Cli::parse().command).await
}
