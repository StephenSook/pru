use std::{env, fs, path::PathBuf, process::ExitCode};

use pru_boundary::{ConsentLedger, GatewayEndpoint, compile_boundary};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pru-boundary: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let ledger_path =
        PathBuf::from(args.next().ok_or(
            "usage: pru-boundary <ledger.json> <output.yaml> [gateway-host] [gateway-port]",
        )?);
    let output_path =
        PathBuf::from(args.next().ok_or(
            "usage: pru-boundary <ledger.json> <output.yaml> [gateway-host] [gateway-port]",
        )?);
    let host = args
        .next()
        .unwrap_or_else(|| "pru-gateway.local".to_owned());
    let port = args.next().map_or(Ok(443), |value| value.parse::<u16>())?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }

    let ledger: ConsentLedger = serde_json::from_slice(&fs::read(&ledger_path)?)?;
    let yaml = compile_boundary(&ledger, &GatewayEndpoint { host, port })?;
    fs::write(&output_path, yaml)?;
    println!(
        "compiled client={} consent_documents={} output={}",
        ledger.client_id,
        ledger.consent_document_ids.len(),
        output_path.display()
    );
    Ok(())
}
