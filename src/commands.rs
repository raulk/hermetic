use std::sync::atomic::Ordering;

use alloy_network::Ethereum;
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use anyhow::{Context as _, Result};

use crate::{
    arti,
    cli::{Command, RailgunWalletArgs, TorArgs},
    railgun::{PopulatedTransaction, RailgunRuntime},
    rpc,
    signer::default_signer_address,
    transport::TOR_CONNECT_CALLS,
};

/// Dispatch a parsed CLI command.
///
/// # Errors
///
/// Returns an error when command validation, Tor bootstrap, Railgun runtime
/// execution, RPC access, or transaction submission fails.
pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Ping { tor, rpc } => ping(tor, rpc).await,
        Command::RuntimeSmoke { workdir } => {
            let mut runtime = RailgunRuntime::new(&workdir).await?;
            runtime_smoke(&mut runtime).await
        }
        Command::LoadWallet { workdir, railgun } => {
            let mut runtime = RailgunRuntime::new(&workdir).await?;
            load_wallet(&mut runtime, &railgun).await
        }
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
            railgun,
            amount_wei,
            dry_run,
        } => {
            let mut runtime = RailgunRuntime::new(&workdir).await?;
            shield(
                &mut runtime,
                tor,
                rpc,
                signer,
                &railgun,
                amount_wei,
                dry_run,
            )
            .await
        }
        Command::Balance {
            tor,
            workdir,
            rpc,
            railgun,
            creation_block,
        } => {
            let rpc_client = bootstrap_rpc_client(tor, rpc).await?;
            let mut runtime = RailgunRuntime::new(&workdir)
                .await?
                .with_rpc_client(rpc_client);
            balance(&mut runtime, &railgun, creation_block).await
        }
        Command::Unshield {
            tor,
            workdir,
            rpc,
            signer,
            railgun,
            amount_wei,
            recipient,
            creation_block,
            dry_run,
        } => {
            let rpc_client = bootstrap_rpc_client(tor, rpc).await?;
            let mut runtime = RailgunRuntime::new(&workdir)
                .await?
                .with_rpc_client(rpc_client.clone());
            let input = UnshieldInput {
                rpc_client,
                signer,
                railgun,
                amount_wei,
                recipient,
                creation_block,
                dry_run,
            };
            unshield(&mut runtime, input).await
        }
    }
}

async fn shield(
    runtime: &mut RailgunRuntime,
    tor: TorArgs,
    rpc: http::Uri,
    signer: crate::signer::PublicSignerArgs,
    railgun: &RailgunWalletArgs,
    amount_wei: U256,
    dry_run: bool,
) -> Result<()> {
    let public_wallet = signer.wallet().await?;
    let railgun_wallet = runtime
        .load_wallet(&railgun.railgun_mnemonic, &railgun.encryption_key, None)
        .await?;
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

    let provider = bootstrap_rpc_client(tor, rpc)
        .await?
        .wallet_provider(public_wallet);
    send_transaction(provider, from, tx, "shield base-token transaction").await
}

async fn balance(
    runtime: &mut RailgunRuntime,
    railgun: &RailgunWalletArgs,
    creation_block: u64,
) -> Result<()> {
    let wallet = runtime
        .load_wallet(
            &railgun.railgun_mnemonic,
            &railgun.encryption_key,
            Some(creation_block),
        )
        .await?;
    let refreshed = runtime.refresh_balance(&wallet.wallet_id).await?;

    println!("wallet_id={}", wallet.wallet_id);
    println!("shielded_address={}", wallet.shielded_address);
    println!("token_address={}", refreshed.token_address);
    println!("balance={}", refreshed.balance);
    println!("spendable_balance={}", refreshed.spendable_balance);
    ensure_tor_was_used("refresh completed")
}

struct UnshieldInput {
    rpc_client: rpc::TorRpcClient,
    signer: crate::signer::PublicSignerArgs,
    railgun: RailgunWalletArgs,
    amount_wei: U256,
    recipient: Option<String>,
    creation_block: u64,
    dry_run: bool,
}

async fn unshield(runtime: &mut RailgunRuntime, input: UnshieldInput) -> Result<()> {
    let public_wallet = input.signer.wallet().await?;
    let from = default_signer_address(&public_wallet);
    let recipient = input.recipient.unwrap_or_else(|| from.to_string());
    let railgun_wallet = runtime
        .load_wallet(
            &input.railgun.railgun_mnemonic,
            &input.railgun.encryption_key,
            Some(input.creation_block),
        )
        .await?;
    let populated = runtime
        .prepare_unshield_base_token(
            &railgun_wallet.wallet_id,
            &recipient,
            &input.railgun.encryption_key,
            &input.amount_wei,
        )
        .await?;

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("to={}", populated.to);
    println!("value={}", populated.value);
    println!("data_len={}", populated.data.len() / 2);
    println!("from={from}");
    println!("recipient={recipient}");
    println!("amount_wei={}", input.amount_wei);
    ensure_tor_was_used("unshield proof completed")?;

    if input.dry_run {
        return Ok(());
    }

    let tx = parse_populated_transaction(&populated)?;
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

async fn runtime_smoke(runtime: &mut RailgunRuntime) -> Result<()> {
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

async fn load_wallet(runtime: &mut RailgunRuntime, railgun: &RailgunWalletArgs) -> Result<()> {
    let wallet = runtime
        .load_wallet(&railgun.railgun_mnemonic, &railgun.encryption_key, None)
        .await?;
    println!("wallet_id={}", wallet.wallet_id);
    println!("shielded_address={}", wallet.shielded_address);
    anyhow::ensure!(
        wallet.shielded_address.starts_with("0zk"),
        "embedded runtime returned non-Railgun address"
    );
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
}

fn parse_populated_transaction(tx: &PopulatedTransaction) -> Result<ParsedTransaction> {
    Ok(ParsedTransaction {
        to: tx.to.parse().context("parsing Railgun tx.to")?,
        data: tx.data.parse().context("parsing Railgun tx.data")?,
        value: U256::from_str_radix(&tx.value, 10).context("parsing Railgun tx.value")?,
    })
}

async fn send_transaction(
    provider: impl Provider<Ethereum>,
    from: Address,
    tx: ParsedTransaction,
    label: &str,
) -> Result<()> {
    let balance = provider
        .get_balance(from)
        .await
        .context("checking signer balance")?;
    println!("public_balance={balance}");
    anyhow::ensure!(
        balance > tx.value,
        "signer has insufficient Sepolia ETH: address {from} balance {balance}, transaction value {}",
        tx.value
    );
    let request = TransactionRequest::default()
        .from(from)
        .to(tx.to)
        .value(tx.value)
        .input(tx.data.into());
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
