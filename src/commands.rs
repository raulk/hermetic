use std::sync::atomic::Ordering;

use alloy_network::Ethereum;
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use anyhow::{Context as _, Result};

use crate::cli::{Command, RailgunImportArgs, TorArgs, WalletCommand, WalletSelectionArgs};
use crate::railgun::manifest::{validate_label, WalletManifest, WalletRecord};
use crate::railgun::{PopulatedTransaction, RailgunRuntime};
use crate::signer::default_signer_address;
use crate::transport::TOR_CONNECT_CALLS;
use crate::{arti, rpc};

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
            let mut runtime = RailgunRuntime::new(&workdir).await?;
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
            let mut runtime = RailgunRuntime::new(&workdir)
                .await?
                .with_rpc_client(rpc_client.clone());
            shield(
                &mut runtime,
                &workdir,
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
            let mut runtime = RailgunRuntime::new(&workdir)
                .await?
                .with_rpc_client(rpc_client);
            balance(&mut runtime, &workdir, &wallet).await
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
            let mut runtime = RailgunRuntime::new(&workdir)
                .await?
                .with_rpc_client(rpc_client.clone());
            let input = UnshieldInput {
                workdir,
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
    signer: crate::signer::PublicSignerArgs,
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
    ensure_tor_was_used("refresh completed")
}

struct UnshieldInput {
    workdir: std::path::PathBuf,
    rpc_client: rpc::TorRpcClient,
    signer: crate::signer::PublicSignerArgs,
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
    ensure_tor_was_used("unshield proof completed")?;

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
    ensure_tor_was_used("provider call completed")
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
            let mut runtime = RailgunRuntime::new(&workdir).await?;
            let wallet = load_wallet(&mut runtime, &railgun).await?;
            upsert_wallet_record(&workdir, &label, wallet)?;
            Ok(())
        }
        WalletCommand::Create {
            workdir,
            label,
            railgun,
        } => {
            validate_label(&label)?;
            let mut runtime = RailgunRuntime::new(&workdir).await?;
            let wallet = runtime.create_wallet(&railgun.encryption_key).await?;
            println!("mnemonic={}", wallet.mnemonic);
            upsert_wallet_record(
                &workdir,
                &label,
                crate::railgun::LoadedWallet {
                    wallet_id: wallet.wallet_id,
                    shielded_address: wallet.shielded_address,
                },
            )?;
            Ok(())
        }
        WalletCommand::List { workdir } => {
            let manifest = WalletManifest::load(&workdir)?;
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
    validate_label(label)?;
    let mut manifest = WalletManifest::load(workdir)?;
    manifest.upsert(WalletRecord {
        label: label.to_owned(),
        wallet_id: wallet.wallet_id,
        shielded_address: wallet.shielded_address,
    });
    manifest.save(workdir)?;
    Ok(())
}

async fn bootstrap_tor(tor: TorArgs) -> Result<crate::arti::ArtiClient> {
    let tor = arti::bootstrap(&tor.tor_state, &tor.tor_cache).await?;
    Ok(arti::isolated_client(&tor))
}

async fn bootstrap_rpc_client(tor: TorArgs, rpc_url: http::Uri) -> Result<rpc::TorRpcClient> {
    Ok(rpc::TorRpcClient::new(bootstrap_tor(tor).await?, rpc_url))
}

struct ParsedTransaction {
    to: Address,
    data: Bytes,
    value: U256,
    gas_limit: Option<u64>,
}

fn parse_populated_transaction(tx: &PopulatedTransaction) -> Result<ParsedTransaction> {
    Ok(ParsedTransaction {
        to: tx.to.parse().context("parsing Railgun tx.to")?,
        data: tx.data.parse().context("parsing Railgun tx.data")?,
        value: U256::from_str_radix(&tx.value, 10).context("parsing Railgun tx.value")?,
        gas_limit: tx
            .gas_limit
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("parsing Railgun tx.gas_limit")?,
    })
}

async fn send_transaction(
    provider: impl Provider<Ethereum>,
    from: Address,
    tx: ParsedTransaction,
    label: &str,
) -> Result<()> {
    let mut request = TransactionRequest::default()
        .from(from)
        .to(tx.to)
        .value(tx.value)
        .input(tx.data.into());
    if let Some(gas_limit) = tx.gas_limit {
        request = request.gas_limit(gas_limit);
    }
    let gas_limit = provider
        .estimate_gas(request.clone())
        .await
        .with_context(|| format!("estimating gas for {label}"))?;
    let gas_price = provider
        .get_gas_price()
        .await
        .context("fetching current gas price")?;
    let max_cost = tx.value + U256::from(gas_limit) * U256::from(gas_price);
    let balance = provider
        .get_balance(from)
        .await
        .context("checking signer balance")?;
    println!("public_balance={balance}");
    println!("estimated_gas={gas_limit}");
    println!("gas_price={gas_price}");
    println!("max_total_cost={max_cost}");
    anyhow::ensure!(
        balance >= max_cost,
        "signer has insufficient Sepolia ETH: address {from} balance {balance}, max transaction cost {max_cost}",
    );
    let pending = provider
        .send_transaction(request)
        .await
        .with_context(|| format!("sending {label}"))?;
    println!("tx_hash={}", pending.tx_hash());
    ensure_tor_was_used("transaction send completed")
}

fn ensure_tor_was_used(action: &str) -> Result<()> {
    let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);
    println!("tor_connect_calls={calls}");
    anyhow::ensure!(calls > 0, "{action} without Tor connector use");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_populated_transaction;
    use crate::railgun::PopulatedTransaction;

    fn populated(
        to: &str,
        data: &str,
        value: &str,
        gas_limit: Option<&str>,
    ) -> PopulatedTransaction {
        PopulatedTransaction {
            to: to.into(),
            data: data.into(),
            value: value.into(),
            gas_limit: gas_limit.map(Into::into),
        }
    }

    // ── happy paths ──────────────────────────────────────────────────────────

    #[test]
    fn parse_populated_transaction_happy_path_with_gas_limit() {
        use alloy_primitives::{address, U256};

        let tx = populated(
            "0x0000000000000000000000000000000000000001",
            "0xdeadbeef",
            "1000000000000000000",
            Some("21000"),
        );
        let parsed = parse_populated_transaction(&tx)
            .expect("valid PopulatedTransaction must parse without error");

        assert_eq!(
            parsed.to,
            address!("0000000000000000000000000000000000000001")
        );
        assert_eq!(parsed.value, U256::from(1_000_000_000_000_000_000_u128));
        assert_eq!(parsed.gas_limit, Some(21_000_u64));
        assert_eq!(parsed.data.len(), 4, "0xdeadbeef is 4 bytes");
    }

    #[test]
    fn parse_populated_transaction_no_gas_limit_is_none() {
        let tx = populated(
            "0x0000000000000000000000000000000000000001",
            "0xdeadbeef",
            "0",
            None,
        );
        let parsed = parse_populated_transaction(&tx)
            .expect("valid PopulatedTransaction without gas_limit must parse");
        assert_eq!(parsed.gas_limit, None);
    }

    // ── error paths ──────────────────────────────────────────────────────────

    #[test]
    fn parse_populated_transaction_invalid_address_returns_err() {
        let tx = populated("not-an-address", "0xdeadbeef", "0", None);
        assert!(parse_populated_transaction(&tx).is_err());
    }

    #[test]
    fn parse_populated_transaction_non_decimal_value_returns_err() {
        let tx = populated(
            "0x0000000000000000000000000000000000000001",
            "0xdeadbeef",
            "not-a-number",
            None,
        );
        assert!(parse_populated_transaction(&tx).is_err());
    }

    #[test]
    fn parse_populated_transaction_malformed_hex_data_returns_err() {
        let tx = populated(
            "0x0000000000000000000000000000000000000001",
            "0xzzzz",
            "0",
            None,
        );
        assert!(parse_populated_transaction(&tx).is_err());
    }
}
