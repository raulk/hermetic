use anyhow::Result;
use clap::Parser;
use hermetic::cli::Cli;
use hermetic::commands;

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
    init_tracing();
    // Installing the global crypto provider can legitimately fail after another
    // test or embedding path installed it first.
    let _ = rustls::crypto::ring::default_provider().install_default();
    commands::run(Cli::parse().command).await
}
