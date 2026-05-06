use anyhow::Result;

use crate::cli::args::{RailgunImportArgs, WalletCommand};
use crate::railgun::manifest::{WalletManifest, WalletRecord};
use crate::railgun::{LoadedWallet, RailgunRuntime};

pub async fn run(command: WalletCommand) -> Result<()> {
    match command {
        WalletCommand::Import {
            workdir,
            label,
            railgun,
        } => {
            let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
            let wallet = import(&mut runtime, &railgun).await?;
            WalletManifest::upsert_record(&workdir.workdir, record(label, wallet))?;
            Ok(())
        }
        WalletCommand::Create {
            workdir,
            label,
            railgun,
        } => {
            let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
            let created = runtime.create_wallet(&railgun.encryption_key).await?;
            println!("mnemonic={}", created.mnemonic);
            WalletManifest::upsert_record(
                &workdir.workdir,
                record(
                    label,
                    LoadedWallet {
                        wallet_id: created.wallet_id,
                        shielded_address: created.shielded_address,
                    },
                ),
            )?;
            Ok(())
        }
        WalletCommand::List { workdir } => {
            let manifest = WalletManifest::load(&workdir.workdir)?;
            for wallet in manifest.wallets {
                println!(
                    "label={} wallet_id={} shielded_address={}",
                    wallet.label, wallet.wallet_id, wallet.shielded_address
                );
            }
            Ok(())
        }
    }
}

async fn import(runtime: &mut RailgunRuntime, railgun: &RailgunImportArgs) -> Result<LoadedWallet> {
    let wallet = runtime
        .load_wallet(&railgun.railgun_mnemonic, &railgun.key.encryption_key)
        .await?;
    println!("wallet_id={}", wallet.wallet_id);
    println!("shielded_address={}", wallet.shielded_address);
    anyhow::ensure!(
        wallet.shielded_address.starts_with("0zk"),
        "embedded runtime returned non-Railgun address"
    );
    Ok(wallet)
}

fn record(label: String, wallet: LoadedWallet) -> WalletRecord {
    WalletRecord {
        label,
        wallet_id: wallet.wallet_id,
        shielded_address: wallet.shielded_address,
    }
}
