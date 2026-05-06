//! Wire types returned by the embedded Railgun runtime to its Rust caller.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Health {
    pub sdk_version: String,
    pub shared_models_version: String,
    pub node_compat: bool,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct PermissionsReport {
    pub fetch_denied: bool,
    pub connect_denied: bool,
    pub node_net_denied: bool,
    pub write_denied: bool,
    pub env_denied: bool,
    pub read_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoadedWallet {
    pub wallet_id: String,
    pub shielded_address: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatedWallet {
    pub wallet_id: String,
    pub shielded_address: String,
    pub mnemonic: String,
}

#[derive(Debug, Deserialize)]
pub struct PopulatedTransaction {
    pub to: String,
    pub data: String,
    pub value: String,
    pub gas_limit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshedBalance {
    pub token_address: String,
    pub balance: String,
    pub spendable_balance: String,
}
