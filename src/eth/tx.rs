//! Parsing and broadcast helpers for transactions populated by the embedded
//! Railgun runtime.

use alloy_network::Ethereum;
use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use anyhow::{Context as _, Result};

use crate::railgun::PopulatedTransaction;

pub struct ParsedTransaction {
    pub to: Address,
    pub data: Bytes,
    pub value: U256,
    pub gas_limit: Option<u64>,
}

/// Parse the strings produced by the JS-side `populate_*` calls into typed
/// Alloy values.
///
/// # Errors
///
/// Returns an error if any field fails to parse.
pub fn parse_populated_transaction(tx: &PopulatedTransaction) -> Result<ParsedTransaction> {
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

/// Estimate gas, check public-signer balance, and broadcast the transaction.
///
/// # Errors
///
/// Returns an error if any RPC call fails or if the signer balance is
/// insufficient to cover gas plus value.
pub async fn send_transaction(
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
