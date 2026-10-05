use std::error::Error;

use chrono::Utc;
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let authority = ConsentAuthority::new();
    let record = ConsentRecord::new(
        "synthetic-live-client",
        ConsentKind::Disclose,
        Some("Nebius Token Factory".to_owned()),
        "answer a synthetic tax-practice question",
        Utc::now(),
        "Synthetic Phase A Signer",
        "phase-a-synthetic-pin",
    )?;
    let token = authority.mint(&record, "phase-a-synthetic-pin")?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "client_id": record.client_id,
            "public_key": authority.verifier().public_key_hex(),
            "token": token
        }))?
    );
    Ok(())
}
