use std::path::PathBuf;

use alloy_primitives::U256;
use clap::{Parser, Subcommand};
use http::Uri;

use crate::signer::PublicSignerArgs;

#[derive(Debug, Parser)]
#[command(name = "hermetic")]
#[command(about = "Railgun transactions with Rust-owned Tor egress")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the selected RPC endpoint is reachable through Tor.
    Ping {
        #[command(flatten)]
        tor: TorArgs,
        #[arg(long, default_value = default_rpc())]
        rpc: Uri,
    },
    /// Verify runtime health, imports, and embedded network isolation.
    Doctor {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
    },
    /// Manage SDK-owned Railgun wallets.
    Wallet {
        #[command(subcommand)]
        command: WalletCommand,
    },
    /// Print the public gas-payer address.
    SignerAddress {
        #[command(flatten)]
        signer: PublicSignerArgs,
    },
    /// Build and optionally send a Sepolia base-token shield transaction.
    Shield {
        #[command(flatten)]
        tor: TorArgs,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = default_rpc())]
        rpc: Uri,
        #[command(flatten)]
        signer: PublicSignerArgs,
        #[command(flatten)]
        wallet: WalletSelectionArgs,
        #[arg(long)]
        amount_wei: U256,
        #[arg(long)]
        dry_run: bool,
    },
    /// Refresh and print the Railgun private base-token balance.
    Balance {
        #[command(flatten)]
        tor: TorArgs,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = default_rpc())]
        rpc: Uri,
        #[command(flatten)]
        wallet: WalletSelectionArgs,
    },
    /// Build and optionally send a Sepolia base-token unshield transaction.
    Unshield {
        #[command(flatten)]
        tor: TorArgs,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = default_rpc())]
        rpc: Uri,
        #[command(flatten)]
        signer: PublicSignerArgs,
        #[command(flatten)]
        wallet: WalletSelectionArgs,
        #[arg(long)]
        amount_wei: U256,
        #[arg(long)]
        recipient: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WalletCommand {
    /// Import a mnemonic into the Railgun SDK artifact store.
    Import {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long)]
        label: String,
        #[command(flatten)]
        railgun: RailgunImportArgs,
    },
    /// Create a new Railgun wallet and print the mnemonic once.
    Create {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long)]
        label: String,
        #[command(flatten)]
        railgun: RailgunKeyArgs,
    },
    /// List known Railgun wallets without exposing secrets.
    List {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
    },
}

#[derive(Clone, Debug, clap::Args)]
pub struct TorArgs {
    #[arg(long, default_value = "./.arti/state")]
    pub tor_state: PathBuf,
    #[arg(long, default_value = "./.arti/cache")]
    pub tor_cache: PathBuf,
}

#[derive(Clone, Debug, clap::Args)]
pub struct RailgunImportArgs {
    #[arg(long, env = "HERMETIC_RAILGUN_MNEMONIC")]
    pub railgun_mnemonic: String,
    #[command(flatten)]
    pub key: RailgunKeyArgs,
}

#[derive(Clone, Debug, clap::Args)]
pub struct RailgunKeyArgs {
    #[arg(long, env = "HERMETIC_RAILGUN_ENCRYPTION_KEY")]
    pub encryption_key: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct WalletSelectionArgs {
    #[arg(long)]
    pub wallet: String,
    #[command(flatten)]
    pub key: RailgunKeyArgs,
}

fn default_rpc() -> &'static str {
    "https://ethereum-sepolia-rpc.publicnode.com"
}
