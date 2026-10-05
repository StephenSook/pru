use std::{env, path::PathBuf};

use pru_consent::ConsentAuthority;
use pru_gateway::{
    GatewayConfig, GatewayState,
    api::{ApiState, api_router},
    router,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let hash_key = decode_hash_key(&env::var("PRU_LEDGER_HASH_KEY")?)?;
    let data_root =
        PathBuf::from(env::var("PRU_DATA_DIR").unwrap_or_else(|_| "data/clients".to_owned()));
    let config = GatewayConfig {
        local_base_url: env::var("PRU_LOCAL_LLM_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8081/v1".to_owned()),
        local_model: env::var("PRU_LOCAL_LLM_MODEL")
            .unwrap_or_else(|_| "nemotron-3-nano-4b".to_owned()),
        local_api_key: env::var("PRU_LOCAL_LLM_API_KEY").ok(),
        token_factory_base_url: "https://api.tokenfactory.nebius.com/v1".to_owned(),
        nebius_api_key: env::var("NEBIUS_API_KEY")?,
        ledger_path: PathBuf::from(
            env::var("PRU_EGRESS_LEDGER").unwrap_or_else(|_| "data/egress.jsonl".to_owned()),
        ),
        client_data_root: Some(data_root.clone()),
        ledger_hash_key: hash_key,
    };
    let authority = ConsentAuthority::new();
    let gateway = GatewayState::new(authority.verifier(), Default::default(), config)?;
    let app =
        api_router(ApiState::new(gateway.clone(), authority, data_root)).merge(router(gateway));
    let bind = env::var("PRU_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn decode_hash_key(value: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| "PRU_LEDGER_HASH_KEY must be 64 hexadecimal characters".into())
}
