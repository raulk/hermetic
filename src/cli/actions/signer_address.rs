use anyhow::Result;

use crate::eth::signer::{default_signer_address, PublicSignerArgs};

pub async fn run(signer: PublicSignerArgs) -> Result<()> {
    let wallet = signer.wallet().await?;
    println!("address={}", default_signer_address(&wallet));
    Ok(())
}
