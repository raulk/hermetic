use alloy_primitives::U256;
use alloy_provider::Provider;
use anyhow::{Context as _, Result};

use crate::cli::{Command, RailgunImportArgs, TorArgs, WalletCommand, WalletSelectionArgs};
use crate::eth::signer::default_signer_address;
use crate::eth::tx::{parse_populated_transaction, send_transaction};
use crate::railgun::manifest::{WalletManifest, WalletRecord};
use crate::railgun::RailgunRuntime;
use crate::{rpc, tor};

/// Dispatch a parsed CLI command.
///
/// # Errors
///
/// Returns an error when command validation, Tor bootstrap, Railgun runtime
/// execution, RPC access, or transaction submission fails.
pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Ping { tor, rpc } => ping(tor, rpc).await,
        Command::Doctor { workdir } => {
            let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
            doctor(&mut runtime).await
        }
        Command::Wallet { command } => wallet_command(command).await,
        Command::SignerAddress { signer } => {
            let wallet = signer.wallet().await?;
            println!("address={}", default_signer_address(&wallet));
            Ok(())
        }
        Command::Shield {
            tor,
            workdir,
            rpc,
            signer,
            wallet,
            amount_wei,
            dry_run,
        } => {
            let rpc_client = bootstrap_rpc_client(tor, rpc).await?;
            let mut runtime = RailgunRuntime::new(&workdir.workdir)
                .await?
                .with_rpc_client(rpc_client.clone());
            shield(
                &mut runtime,
                &workdir.workdir,
                rpc_client,
                signer,
                &wallet,
                amount_wei,
                dry_run,
            )
            .await
        }
        Command::Balance {
            tor,
            workdir,
            rpc,
            wallet,
        } => {
            let rpc_client = bootstrap_rpc_client(tor, rpc).await?;
            let mut runtime = RailgunRuntime::new(&workdir.workdir)
                .await?
                .with_rpc_client(rpc_client);
            balance(&mut runtime, &workdir.workdir, &wallet).await
        }
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
            let rpc_client = bootstrap_rpc_client(tor, rpc).await?;
            let mut runtime = RailgunRuntime::new(&workdir.workdir)
                .await?
                .with_rpc_client(rpc_client.clone());
            let input = UnshieldInput {
                workdir: workdir.workdir,
                rpc_client,
                signer,
                wallet,
                amount_wei,
                recipient,
                dry_run,
            };
            unshield(&mut runtime, input).await
        }
    }
}

async fn shield(
    runtime: &mut RailgunRuntime,
    workdir: &std::path::Path,
    rpc_client: rpc::TorRpcClient,
    signer: crate::eth::signer::PublicSignerArgs,
    wallet: &WalletSelectionArgs,
    amount_wei: U256,
    dry_run: bool,
) -> Result<()> {
    let public_wallet = signer.wallet().await?;
    let railgun_wallet = load_selected_wallet(runtime, workdir, wallet).await?;
    let populated = runtime
        .populate_shield_base_token(&railgun_wallet.shielded_address, &amount_wei)
        .await?;
    let tx = parse_populated_transaction(&populated)?;
    let from = default_signer_address(&public_wallet);

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("to={}", tx.to);
    println!("value={}", tx.value);
    println!("data_len={}", tx.data.len());
    println!("from={from}");

    if dry_run {
        return Ok(());
    }

    let provider = rpc_client.wallet_provider(public_wallet);
    send_transaction(provider, from, tx, "shield base-token transaction").await
}

async fn balance(
    runtime: &mut RailgunRuntime,
    workdir: &std::path::Path,
    wallet: &WalletSelectionArgs,
) -> Result<()> {
    let railgun_wallet = load_selected_wallet(runtime, workdir, wallet).await?;
    let refreshed = runtime.refresh_balance(&railgun_wallet.wallet_id).await?;

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("token_address={}", refreshed.token_address);
    println!("balance={}", refreshed.balance);
    println!("spendable_balance={}", refreshed.spendable_balance);
    Ok(())
}

struct UnshieldInput {
    workdir: std::path::PathBuf,
    rpc_client: rpc::TorRpcClient,
    signer: crate::eth::signer::PublicSignerArgs,
    wallet: WalletSelectionArgs,
    amount_wei: U256,
    recipient: Option<String>,
    dry_run: bool,
}

async fn unshield(runtime: &mut RailgunRuntime, input: UnshieldInput) -> Result<()> {
    let public_wallet = input.signer.wallet().await?;
    let from = default_signer_address(&public_wallet);
    let recipient = input.recipient.unwrap_or_else(|| from.to_string());
    let railgun_wallet = load_selected_wallet(runtime, &input.workdir, &input.wallet).await?;
    let populated = runtime
        .prepare_unshield_base_token(
            &railgun_wallet.wallet_id,
            &recipient,
            &input.wallet.key.encryption_key,
            &input.amount_wei,
        )
        .await?;
    let tx = parse_populated_transaction(&populated)?;

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("to={}", populated.to);
    println!("value={}", populated.value);
    println!("data_len={}", tx.data.len());
    println!("from={from}");
    println!("recipient={recipient}");
    println!("amount_wei={}", input.amount_wei);

    if input.dry_run {
        return Ok(());
    }

    let provider = input.rpc_client.wallet_provider(public_wallet);
    send_transaction(provider, from, tx, "unshield base-token transaction").await
}

async fn ping(tor: TorArgs, rpc_url: http::Uri) -> Result<()> {
    let rpc_client = bootstrap_rpc_client(tor, rpc_url).await?;
    let provider = rpc_client.provider();
    let chain_id = provider.get_chain_id().await.context("eth_chainId")?;
    let block_number = provider
        .get_block_number()
        .await
        .context("eth_blockNumber")?;

    println!("chain_id={chain_id}");
    println!("block_number={block_number}");
    Ok(())
}

async fn doctor(runtime: &mut RailgunRuntime) -> Result<()> {
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

async fn wallet_command(command: WalletCommand) -> Result<()> {
    match command {
        WalletCommand::Import {
            workdir,
            label,
            railgun,
        } => {
            let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
            let wallet = load_wallet(&mut runtime, &railgun).await?;
            upsert_wallet_record(&workdir.workdir, &label, wallet)?;
            Ok(())
        }
        WalletCommand::Create {
            workdir,
            label,
            railgun,
        } => {
            let mut runtime = RailgunRuntime::new(&workdir.workdir).await?;
            let wallet = runtime.create_wallet(&railgun.encryption_key).await?;
            println!("mnemonic={}", wallet.mnemonic);
            upsert_wallet_record(
                &workdir.workdir,
                &label,
                crate::railgun::LoadedWallet {
                    wallet_id: wallet.wallet_id,
                    shielded_address: wallet.shielded_address,
                },
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

async fn load_wallet(
    runtime: &mut RailgunRuntime,
    railgun: &RailgunImportArgs,
) -> Result<crate::railgun::LoadedWallet> {
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

async fn load_selected_wallet(
    runtime: &mut RailgunRuntime,
    workdir: &std::path::Path,
    selection: &WalletSelectionArgs,
) -> Result<crate::railgun::LoadedWallet> {
    let manifest = WalletManifest::load(workdir)?;
    let record = manifest.select(&selection.wallet)?;
    runtime
        .load_wallet_by_id(&record.wallet_id, &selection.key.encryption_key)
        .await
}

fn upsert_wallet_record(
    workdir: &std::path::Path,
    label: &str,
    wallet: crate::railgun::LoadedWallet,
) -> Result<()> {
    let mut manifest = WalletManifest::load(workdir)?;
    manifest.upsert(WalletRecord {
        label: label.to_owned(),
        wallet_id: wallet.wallet_id,
        shielded_address: wallet.shielded_address,
    });
    manifest.save(workdir)?;
    Ok(())
}

async fn bootstrap_tor(tor: TorArgs) -> Result<crate::tor::ArtiClient> {
    let tor = tor::bootstrap(&tor.tor_state, &tor.tor_cache).await?;
    Ok(tor.isolated_client())
}

async fn bootstrap_rpc_client(tor: TorArgs, rpc_url: http::Uri) -> Result<rpc::TorRpcClient> {
    Ok(rpc::TorRpcClient::new(bootstrap_tor(tor).await?, rpc_url))
}
