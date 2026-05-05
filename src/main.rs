use std::{path::PathBuf, sync::atomic::Ordering};

use alloy_network::{Ethereum, EthereumWallet, NetworkWallet};
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_ledger::{HDPath, LedgerSigner};
use alloy_signer_local::PrivateKeySigner;
use anyhow::Context as _;
use clap::{Args, Parser, Subcommand};
#[cfg(feature = "deno-runtime")]
use undercover::embedded::EmbeddedRailgun;
#[cfg(feature = "deno-runtime")]
use undercover::sidecar::PermissionSmoke;
use undercover::{
    arti::{self, IsolationLabel},
    rpc,
    sidecar::{Health, LoadedWallet, PopulatedTransaction, RefreshedBalance, Sidecar},
    transport::TOR_CONNECT_CALLS,
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
        #[cfg(feature = "deno-runtime")]
        #[arg(long)]
        embedded: bool,
    },
    LoadWalletSmoke {
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[cfg(feature = "deno-runtime")]
        #[arg(long)]
        embedded: bool,
        #[arg(long, env = "UNDERCOVER_RAILGUN_MNEMONIC")]
        railgun_mnemonic: String,
        #[arg(
            long,
            default_value = "0101010101010101010101010101010101010101010101010101010101010101"
        )]
        encryption_key: String,
    },
    SignerAddress {
        #[command(flatten)]
        signer: PublicSignerArgs,
    },
    ShieldBaseToken {
        #[arg(long)]
        rpc: http::Uri,
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
        #[cfg(feature = "deno-runtime")]
        #[arg(long)]
        embedded: bool,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
        #[command(flatten)]
        signer: PublicSignerArgs,
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
        #[cfg(feature = "deno-runtime")]
        #[arg(long)]
        embedded: bool,
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
        #[cfg(feature = "deno-runtime")]
        #[arg(long)]
        embedded: bool,
        #[arg(long, default_value = "./.arti/state")]
        arti_state: PathBuf,
        #[arg(long, default_value = "./.arti/cache")]
        arti_cache: PathBuf,
        #[command(flatten)]
        signer: PublicSignerArgs,
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

#[derive(Clone, Debug, Args)]
struct PublicSignerArgs {
    #[arg(long, env = "UNDERCOVER_PRIVATE_KEY", conflicts_with = "ledger")]
    private_key: Option<String>,
    #[arg(long)]
    ledger: bool,
    #[arg(long, default_value_t = 11155111)]
    chain_id: u64,
    #[arg(long, default_value_t = 0)]
    ledger_index: usize,
    #[arg(long)]
    ledger_path: Option<String>,
}

impl PublicSignerArgs {
    async fn wallet(&self) -> anyhow::Result<EthereumWallet> {
        if let Some(private_key) = &self.private_key {
            let signer: PrivateKeySigner = private_key.parse().context("parsing private key")?;
            return Ok(EthereumWallet::from(signer));
        }
        anyhow::ensure!(
            self.ledger,
            "choose a public signer with --private-key/UNDERCOVER_PRIVATE_KEY or --ledger"
        );
        let path = self
            .ledger_path
            .as_ref()
            .map(|path| HDPath::Other(path.clone()))
            .unwrap_or(HDPath::LedgerLive(self.ledger_index));
        let signer = LedgerSigner::new(path, Some(self.chain_id))
            .await
            .context("connecting to Ledger Ethereum app")?;
        Ok(EthereumWallet::from(signer))
    }
}

fn default_signer_address(wallet: &EthereumWallet) -> Address {
    <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(wallet)
}

#[tokio::main(flavor = "current_thread")]
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
            let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);

            println!("chain_id={chain_id}");
            println!("block_number={block_number}");
            println!("tor_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "provider call completed without Tor connector use"
            );
        }
        Command::SidecarSmoke {
            workdir,
            #[cfg(feature = "deno-runtime")]
            embedded,
        } => {
            #[cfg(feature = "deno-runtime")]
            if embedded {
                let listener = std::net::TcpListener::bind("127.0.0.1:0")
                    .context("binding local permission probe socket")?;
                let node_net_port = listener.local_addr()?.port();
                let mut runtime = EmbeddedRailgun::new(&workdir).await?;
                let health: Health = runtime.call("health", serde_json::json!({})).await?;
                println!("sdk_version={}", health.sdk_version);
                println!("shared_models_version={}", health.shared_models_version);
                println!("node_compat={}", health.node_compat);
                anyhow::ensure!(health.node_compat, "embedded SDK imports did not load");
                let smoke: PermissionSmoke = runtime
                    .call(
                        "sidecar-permissions-smoke",
                        serde_json::json!({ "node_net_port": node_net_port }),
                    )
                    .await?;
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
                return Ok(());
            }
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
            #[cfg(feature = "deno-runtime")]
            embedded,
            railgun_mnemonic,
            encryption_key,
        } => {
            #[cfg(feature = "deno-runtime")]
            if embedded {
                let mut runtime = EmbeddedRailgun::new(&workdir).await?;
                let wallet: LoadedWallet = runtime
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
                    "embedded runtime returned non-Railgun address"
                );
                return Ok(());
            }
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
        Command::SignerAddress { signer } => {
            let wallet = signer.wallet().await?;
            println!("address={}", default_signer_address(&wallet));
        }
        Command::ShieldBaseToken {
            rpc,
            workdir,
            #[cfg(feature = "deno-runtime")]
            embedded,
            arti_state,
            arti_cache,
            signer,
            railgun_mnemonic,
            encryption_key,
            amount_wei,
            dry_run,
        } => {
            let public_wallet = signer.wallet().await?;
            #[cfg(feature = "deno-runtime")]
            let (railgun_wallet, populated) = if embedded {
                let mut runtime = EmbeddedRailgun::new(&workdir).await?;
                let wallet: LoadedWallet = runtime
                    .call(
                        "load_wallet",
                        serde_json::json!({
                            "mnemonic": railgun_mnemonic,
                            "encryption_key": encryption_key,
                        }),
                    )
                    .await?;
                let populated: PopulatedTransaction = runtime
                    .call(
                        "populate_shield_base_token",
                        serde_json::json!({
                            "railgun_address": wallet.shielded_address,
                            "amount_wei": amount_wei.to_string(),
                        }),
                    )
                    .await?;
                (wallet, populated)
            } else {
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
                (wallet, populated)
            };
            #[cfg(not(feature = "deno-runtime"))]
            let (railgun_wallet, populated) = {
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
                (wallet, populated)
            };

            let to: Address = populated.to.parse().context("parsing sidecar tx.to")?;
            let data: Bytes = populated.data.parse().context("parsing sidecar tx.data")?;
            let value =
                U256::from_str_radix(&populated.value, 10).context("parsing sidecar tx.value")?;

            println!("wallet_id={}", railgun_wallet.wallet_id);
            println!("shielded_address={}", railgun_wallet.shielded_address);
            println!("to={to}");
            println!("value={value}");
            println!("data_len={}", data.len());
            let from = default_signer_address(&public_wallet);
            println!("from={from}");

            if dry_run {
                return Ok(());
            }

            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            let provider = rpc::wallet_provider(tor, rpc, public_wallet);
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
            let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("tor_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "transaction send completed without Tor connector use"
            );
        }
        Command::RefreshBalance {
            rpc,
            workdir,
            #[cfg(feature = "deno-runtime")]
            embedded,
            arti_state,
            arti_cache,
            railgun_mnemonic,
            encryption_key,
            creation_block,
        } => {
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            #[cfg(feature = "deno-runtime")]
            let (wallet, refreshed) = if embedded {
                let mut runtime = EmbeddedRailgun::new(&workdir).await?;
                let wallet: LoadedWallet = runtime
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
                    runtime.call_with_reverse_rpc(
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
                (wallet, refreshed)
            } else {
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
                sidecar.shutdown().await?;
                (wallet, refreshed)
            };
            #[cfg(not(feature = "deno-runtime"))]
            let (wallet, refreshed) = {
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
                sidecar.shutdown().await?;
                (wallet, refreshed)
            };
            println!("wallet_id={}", wallet.wallet_id);
            println!("shielded_address={}", wallet.shielded_address);
            println!("token_address={}", refreshed.token_address);
            println!("balance={}", refreshed.balance);
            println!("spendable_balance={}", refreshed.spendable_balance);
            let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("tor_connect_calls={calls}");
            anyhow::ensure!(calls > 0, "refresh completed without Tor connector use");
        }
        Command::UnshieldBaseToken {
            rpc,
            workdir,
            #[cfg(feature = "deno-runtime")]
            embedded,
            arti_state,
            arti_cache,
            signer,
            railgun_mnemonic,
            encryption_key,
            amount_wei,
            recipient,
            creation_block,
            dry_run,
        } => {
            let public_wallet = signer.wallet().await?;
            let from = default_signer_address(&public_wallet);
            let recipient = recipient.unwrap_or_else(|| from.to_string());
            let tor = arti::bootstrap(&arti_state, &arti_cache).await?;
            let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
            #[cfg(feature = "deno-runtime")]
            let (railgun_wallet, populated) = if embedded {
                let mut runtime = EmbeddedRailgun::new(&workdir).await?;
                let wallet: LoadedWallet = runtime
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
                    runtime.call_with_reverse_rpc(
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
                (wallet, populated)
            } else {
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
                sidecar.shutdown().await?;
                (wallet, populated)
            };
            #[cfg(not(feature = "deno-runtime"))]
            let (railgun_wallet, populated) = {
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
                sidecar.shutdown().await?;
                (wallet, populated)
            };
            println!("wallet_id={}", railgun_wallet.wallet_id);
            println!("shielded_address={}", railgun_wallet.shielded_address);
            println!("to={}", populated.to);
            println!("value={}", populated.value);
            println!("data_len={}", populated.data.len() / 2);
            println!("from={from}");
            println!("recipient={recipient}");
            println!("amount_wei={amount_wei}");
            let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("tor_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "unshield proof completed without Tor connector use"
            );
            if dry_run {
                return Ok(());
            }

            let to: Address = populated.to.parse().context("parsing sidecar tx.to")?;
            let data: Bytes = populated.data.parse().context("parsing sidecar tx.data")?;
            let value =
                U256::from_str_radix(&populated.value, 10).context("parsing sidecar tx.value")?;
            let provider = rpc::wallet_provider(tor, rpc, public_wallet);
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
            let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);
            println!("tor_connect_calls={calls}");
            anyhow::ensure!(
                calls > 0,
                "transaction send completed without Tor connector use"
            );
        }
    }

    Ok(())
}
