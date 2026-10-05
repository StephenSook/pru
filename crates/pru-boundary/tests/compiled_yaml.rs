use std::{fs, path::PathBuf};

use pru_boundary::{ConsentLedger, GatewayEndpoint, compile_boundary};
use yaml_rust2::YamlLoader;

#[test]
fn compiled_boundary_is_one_valid_yaml_document() {
    let source = compile_boundary(
        &ConsentLedger {
            client_id: "synthetic-client-test".to_owned(),
            consent_document_ids: vec![],
        },
        &GatewayEndpoint {
            host: "pru-gateway.local".to_owned(),
            port: 443,
        },
    )
    .unwrap();
    let documents = YamlLoader::load_from_str(&source).unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["version"].as_i64(), Some(1));
    assert_eq!(
        documents[0]["network_policies"]["pru_gateway"]["endpoints"][0]["protocol"].as_str(),
        Some("rest")
    );
}

#[test]
fn committed_example_matches_compiler() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ledger: ConsentLedger =
        serde_json::from_slice(&fs::read(root.join("eval/ledgers/synthetic-client.json")).unwrap())
            .unwrap();
    let actual = compile_boundary(
        &ledger,
        &GatewayEndpoint {
            host: "pru-gateway.local".to_owned(),
            port: 443,
        },
    )
    .unwrap();
    let committed = fs::read_to_string(root.join("eval/boundaries/synthetic-client.yaml")).unwrap();
    assert_eq!(actual, committed);
}
