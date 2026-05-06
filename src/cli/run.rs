//! Command dispatcher: parses arguments are matched to per-command bodies in
//! `cli::actions`.

use anyhow::Result;

use super::actions::{balance, doctor, ping, shield, signer_address, unshield, wallet};
use super::args::Command;

/// Dispatch a parsed CLI command.
///
/// # Errors
///
/// Returns an error when command validation, Tor bootstrap, Railgun runtime
/// execution, RPC access, or transaction submission fails.
pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Ping { tor, rpc } => ping::run(tor, rpc).await,
        Command::Doctor { workdir } => doctor::run(workdir).await,
        Command::Wallet { command } => wallet::run(command).await,
        Command::SignerAddress { signer } => signer_address::run(signer).await,
        Command::Shield {
            tor,
            workdir,
            rpc,
            signer,
            wallet,
            amount_wei,
            dry_run,
        } => shield::run(tor, workdir, rpc, signer, wallet, amount_wei, dry_run).await,
        Command::Balance {
            tor,
            workdir,
            rpc,
            wallet,
        } => balance::run(tor, workdir, rpc, wallet).await,
        Command::Unshield {
            tor,
            workdir,
            rpc,
            signer,
            wallet,
            amount_wei,
            recipient,
            dry_run,
        } => {
            unshield::run(
                tor, workdir, rpc, signer, wallet, amount_wei, recipient, dry_run,
            )
            .await
        }
    }
}
