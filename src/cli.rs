use std::path::PathBuf;

use alloy_primitives::U256;
use clap::{Parser, Subcommand};
use http::Uri;

use crate::signer::PublicSignerArgs;

#[derive(Debug, Parser)]
#[command(name = "undercover")]
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
    /// Verify that the embedded Railgun runtime loads and has no network access.
    RuntimeSmoke {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
    },
    /// Load a Railgun wallet and print its shielded address.
    LoadWallet {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[command(flatten)]
        railgun: RailgunWalletArgs,
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
        railgun: RailgunWalletArgs,
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
        railgun: RailgunWalletArgs,
        #[arg(long, default_value_t = 0)]
        creation_block: u64,
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
        railgun: RailgunWalletArgs,
        #[arg(long)]
        amount_wei: U256,
        #[arg(long)]
        recipient: Option<String>,
        #[arg(long, default_value_t = 0)]
        creation_block: u64,
        #[arg(long)]
        dry_run: bool,
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
pub struct RailgunWalletArgs {
    #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
    pub railgun_mnemonic: String,
    #[arg(
        long,
        default_value = "0101010101010101010101010101010101010101010101010101010101010101"
    )]
    pub encryption_key: String,
}

fn default_rpc() -> &'static str {
    "https://ethereum-sepolia-rpc.publicnode.com"
}
