use std::{path::PathBuf, sync::atomic::Ordering};

use alloy_network::EthereumWallet;
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use anyhow::Context as _;
use clap::{Parser, Subcommand};
use undercover::{
    arti::{self, IsolationLabel},
    rpc,
    sidecar::{Health, LoadedWallet, PopulatedTransaction, RefreshedBalance, Sidecar},
    transport::ARTI_CONNECT_CALLS,
};

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("undercover=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

#[derive(Debug, Parser)]
#[command(name = "undercover")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Ping {
        #[arg(long)]
        rpc: http::Uri,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
    },
    SidecarSmoke {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
    },
    LoadWalletSmoke {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
        railgun_mnemonic: String,
        #[arg(
            long,
            default_value = "0101010101010101010101010101010101010101010101010101010101010101"
        )]
        encryption_key: String,
    },
    SignerAddress {
        #[arg(long, env = "UNDERCOVER_PRIVATE_KEY")]
        private_key: String,
    },
    ShieldBaseToken {
        #[arg(long)]
        rpc: http::Uri,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
        #[arg(long, env = "UNDERCOVER_PRIVATE_KEY")]
        private_key: String,
        #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
        railgun_mnemonic: String,
        #[arg(
            long,
            default_value = "0101010101010101010101010101010101010101010101010101010101010101"
        )]
        encryption_key: String,
        #[arg(long)]
        amount_wei: U256,
        #[arg(long)]
        dry_run: bool,
    },
    RefreshBalance {
        #[arg(long)]
        rpc: http::Uri,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
        #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
        railgun_mnemonic: String,
        #[arg(
            long,
            default_value = "0101010101010101010101010101010101010101010101010101010101010101"
        )]
        encryption_key: String,
        #[arg(long, default_value_t = 0)]
        creation_block: u64,
    },
    UnshieldBaseToken {
        #[arg(long)]
        rpc: http::Uri,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
        #[arg(long, env = "UNDERCOVER_PRIVATE_KEY")]
        private_key: String,
        #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
        railgun_mnemonic: String,
        #[arg(
            long,
            default_value = "0101010101010101010101010101010101010101010101010101010101010101"
        )]
        encryption_key: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let _ = rustls::crypto::ring::default_provider().install_default();

    match Cli::parse().command {
        Command::Ping {
            rpc,
            arti_state,
            arti_cache,
        } => {
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            let provider = rpc::provider(tor, rpc);
            let chain_id = provider.get_chain_id().await.context("eth_chainId")?;
            let block_number = provider
                .get_block_number()
                .await
                .context("eth_blockNumber")?;
            let calls = ARTI_CONNECT_CALLS.load(Ordering::SeqCst);

            println!("chain_id={chain_id}");
            println!("block_number={block_number}");
            println!("arti_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "provider call completed without ArtiConnector use"
            );
        }
        Command::SidecarSmoke { workdir } => {
            let mut sidecar = Sidecar::spawn(&workdir).await?;
            let health: Health = sidecar.call("health", serde_json::json!({})).await?;
            println!("sdk_version={}", health.sdk_version);
            println!("shared_models_version={}", health.shared_models_version);
            println!("node_compat={}", health.node_compat);
            anyhow::ensure!(health.node_compat, "sidecar SDK imports did not load");
            sidecar.shutdown().await?;
        }
        Command::LoadWalletSmoke {
            workdir,
            railgun_mnemonic,
            encryption_key,
        } => {
            let mut sidecar = Sidecar::spawn(&workdir).await?;
            let wallet: LoadedWallet = sidecar
                .call(
                    "load_wallet",
                    serde_json::json!({
                        "mnemonic": railgun_mnemonic,
                        "encryption_key": encryption_key,
                    }),
                )
                .await?;
            println!("wallet_id={}", wallet.wallet_id);
            println!("shielded_address={}", wallet.shielded_address);
            anyhow::ensure!(
                wallet.shielded_address.starts_with("0zk"),
                "sidecar returned non-Railgun address"
            );
            sidecar.shutdown().await?;
        }
        Command::SignerAddress { private_key } => {
            let private_key: PrivateKeySigner =
                private_key.parse().context("parsing private key")?;
            println!("address={}", private_key.address());
        }
        Command::ShieldBaseToken {
            rpc,
            workdir,
            arti_state,
            arti_cache,
            private_key,
            railgun_mnemonic,
            encryption_key,
            amount_wei,
            dry_run,
        } => {
            let private_key: PrivateKeySigner =
                private_key.parse().context("parsing private key")?;
            let mut sidecar = Sidecar::spawn(&workdir).await?;
            let wallet: LoadedWallet = sidecar
                .call(
                    "load_wallet",
                    serde_json::json!({
                        "mnemonic": railgun_mnemonic,
                        "encryption_key": encryption_key,
                    }),
                )
                .await?;
            let populated: PopulatedTransaction = sidecar
                .call(
                    "populate_shield_base_token",
                    serde_json::json!({
                        "railgun_address": wallet.shielded_address,
                        "amount_wei": amount_wei.to_string(),
                    }),
                )
                .await?;
            sidecar.shutdown().await?;

            let to: Address = populated.to.parse().context("parsing sidecar tx.to")?;
            let data: Bytes = populated.data.parse().context("parsing sidecar tx.data")?;
            let value =
                U256::from_str_radix(&populated.value, 10).context("parsing sidecar tx.value")?;

            println!("wallet_id={}", wallet.wallet_id);
            println!("shielded_address={}", wallet.shielded_address);
            println!("to={to}");
            println!("value={value}");
            println!("data_len={}", data.len());
            let from = private_key.address();
            println!("from={from}");

            if dry_run {
                return Ok(());
            }

            let wallet = EthereumWallet::from(private_key);
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            let provider = rpc::wallet_provider(tor, rpc, wallet);
            let balance = provider
                .get_balance(from)
                .await
                .context("checking signer balance")?;
            println!("balance={balance}");
            anyhow::ensure!(
                balance > value,
                "signer has insufficient Sepolia ETH: address {from} balance {balance}, transaction value {value}; fund this address and rerun the same command"
            );
            let tx = TransactionRequest::default()
                .from(from)
                .to(to)
                .value(value)
                .input(data.into());
            let pending = provider
                .send_transaction(tx)
                .await
                .context("sending shield base-token transaction")?;
            println!("tx_hash={}", pending.tx_hash());
            let calls = ARTI_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("arti_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "transaction send completed without ArtiConnector use"
            );
        }
        Command::RefreshBalance {
            rpc,
            workdir,
            arti_state,
            arti_cache,
            railgun_mnemonic,
            encryption_key,
            creation_block,
        } => {
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            let mut sidecar = Sidecar::spawn(&workdir).await?;
            let wallet: LoadedWallet = sidecar
                .call(
                    "load_wallet",
                    serde_json::json!({
                        "mnemonic": railgun_mnemonic,
                        "encryption_key": encryption_key,
                        "creation_block_numbers": {
                            "Ethereum_Sepolia": creation_block,
                        },
                    }),
                )
                .await?;
            let refreshed: RefreshedBalance = tokio::time::timeout(
                std::time::Duration::from_secs(180),
                sidecar.call_with_reverse_rpc(
                    "refresh_balance",
                    serde_json::json!({
                        "wallet_id": wallet.wallet_id,
                    }),
                    tor,
                    rpc,
                ),
            )
            .await
            .context("timed out refreshing Railgun balance")??;
            println!("wallet_id={}", wallet.wallet_id);
            println!("shielded_address={}", wallet.shielded_address);
            println!("token_address={}", refreshed.token_address);
            println!("balance={}", refreshed.balance);
            println!("spendable_balance={}", refreshed.spendable_balance);
            let calls = ARTI_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("arti_connect_calls={calls}");
            anyhow::ensure!(calls > 0, "refresh completed without ArtiConnector use");
            sidecar.shutdown().await?;
        }
        Command::UnshieldBaseToken {
            rpc,
            workdir,
            arti_state,
            arti_cache,
            private_key,
            railgun_mnemonic,
            encryption_key,
            amount_wei,
            recipient,
            creation_block,
            dry_run,
        } => {
            let private_key: PrivateKeySigner =
                private_key.parse().context("parsing private key")?;
            let from = private_key.address();
            let recipient = recipient.unwrap_or_else(|| from.to_string());
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            let mut sidecar = Sidecar::spawn(&workdir).await?;
            let wallet: LoadedWallet = sidecar
                .call(
                    "load_wallet",
                    serde_json::json!({
                        "mnemonic": railgun_mnemonic,
                        "encryption_key": encryption_key,
                        "creation_block_numbers": {
                            "Ethereum_Sepolia": creation_block,
                        },
                    }),
                )
                .await?;
            let populated: PopulatedTransaction = tokio::time::timeout(
                std::time::Duration::from_secs(900),
                sidecar.call_with_reverse_rpc(
                    "populate_unshield_base_token",
                    serde_json::json!({
                        "wallet_id": wallet.wallet_id,
                        "public_wallet_address": recipient,
                        "encryption_key": encryption_key,
                        "amount_wei": amount_wei.to_string(),
                    }),
                    tor.clone(),
                    rpc.clone(),
                ),
            )
            .await
            .context("timed out proving Railgun unshield")??;
            println!("wallet_id={}", wallet.wallet_id);
            println!("shielded_address={}", wallet.shielded_address);
            println!("to={}", populated.to);
            println!("value={}", populated.value);
            println!("data_len={}", populated.data.len() / 2);
            println!("from={from}");
            println!("recipient={recipient}");
            println!("amount_wei={amount_wei}");
            let calls = ARTI_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("arti_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "unshield proof completed without ArtiConnector use"
            );
            sidecar.shutdown().await?;

            if dry_run {
                return Ok(());
            }

            let to: Address = populated.to.parse().context("parsing sidecar tx.to")?;
            let data: Bytes = populated.data.parse().context("parsing sidecar tx.data")?;
            let value =
                U256::from_str_radix(&populated.value, 10).context("parsing sidecar tx.value")?;
            let wallet = EthereumWallet::from(private_key);
            let provider = rpc::wallet_provider(tor, rpc, wallet);
            let balance = provider
                .get_balance(from)
                .await
                .context("checking signer balance")?;
            println!("public_balance={balance}");
            anyhow::ensure!(
                balance > value,
                "signer has insufficient Sepolia ETH: address {from} balance {balance}, transaction value {value}"
            );
            let tx = TransactionRequest::default()
                .from(from)
                .to(to)
                .value(value)
                .input(data.into());
            let pending = provider
                .send_transaction(tx)
                .await
                .context("sending unshield base-token transaction")?;
            println!("tx_hash={}", pending.tx_hash());
            let calls = ARTI_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("arti_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "transaction send completed without ArtiConnector use"
            );
        }
    }

    Ok(())
}
