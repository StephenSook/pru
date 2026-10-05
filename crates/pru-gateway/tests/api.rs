use std::path::PathBuf;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use chrono::{TimeDelta, Utc};
use pru_consent::{ConsentAuthority, RevocationList};
use pru_gateway::{
    GatewayConfig, GatewayState,
    api::{ApiState, api_router},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const CLIENT_A: &str = "demo-avery";
const CLIENT_B: &str = "demo-riley";

struct Harness {
    app: Router,
    root: PathBuf,
    _temp: TempDir,
}

fn harness() -> Harness {
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("clients");
    let authority = ConsentAuthority::new();
    let gateway = GatewayState::new(
        authority.verifier(),
        RevocationList::default(),
        GatewayConfig {
            local_base_url: "http://127.0.0.1:1/v1".to_owned(),
            local_model: "synthetic-local-model".to_owned(),
            local_api_key: None,
            token_factory_base_url: "http://127.0.0.1:2/v1".to_owned(),
            nebius_api_key: "synthetic-test-key".to_owned(),
            ledger_path: temp.path().join("legacy.jsonl"),
            client_data_root: Some(root.clone()),
            ledger_hash_key: [11; 32],
        },
    )
    .expect("gateway state");
    let api_state = ApiState::new(gateway, authority, root.clone());
    api_state.reset_demo().expect("seed demo clients");
    Harness {
        app: api_router(api_state),
        root,
        _temp: temp,
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let bytes = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        serde_json::to_vec(&body).expect("request json")
    } else {
        Vec::new()
    };
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(bytes)).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("response body");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 response");
    assert!(!text.contains("102-00-1001"));
    assert!(!text.contains("902-12-1002"));
    let value = serde_json::from_slice(&bytes).expect("JSON response");
    (status, value)
}

fn consent_body(recipient: Option<&str>) -> Value {
    json!({
        "kind": if recipient.is_some() { "disclose" } else { "use" },
        "recipient": recipient,
        "purpose": "prepare the synthetic return",
        "expires_at": (Utc::now() + TimeDelta::days(30)).to_rfc3339(),
        "pin": "53179"
    })
}

#[tokio::test]
async fn health_endpoint_reports_ok() {
    let harness = harness();
    let (status, body) = request(&harness.app, Method::GET, "/healthz", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"status": "ok"}));
}

#[tokio::test]
async fn consent_endpoints_hash_pin_list_redacted_fields_and_revoke() {
    let harness = harness();
    let (status, created) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        Some(consent_body(Some("client-a@example.com"))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["kind"], "disclose");
    assert_eq!(created["revoked"], false);
    assert!(created.get("token").is_none());
    assert!(created.get("pin_hash").is_none());

    let stored = std::fs::read_to_string(harness.root.join(CLIENT_A).join("consents.json"))
        .expect("stored consents");
    assert!(!stored.contains("53179"));
    assert!(stored.contains("$argon2id$"));

    let (status, listed) = request(
        &harness.app,
        Method::GET,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.as_array().expect("consent list").len(), 1);

    let id = created["id"].as_str().expect("consent id");
    let (status, revoked) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT_A}/consents/{id}/revoke"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["revoked"], true);
}

#[tokio::test]
async fn client_a_records_are_not_visible_through_client_b_routes() {
    let harness = harness();
    let (status, created) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        Some(consent_body(None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (_, client_a) = request(
        &harness.app,
        Method::GET,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        None,
    )
    .await;
    let (_, client_b) = request(
        &harness.app,
        Method::GET,
        &format!("/v1/clients/{CLIENT_B}/consents"),
        None,
    )
    .await;
    assert_eq!(client_a.as_array().expect("client A list").len(), 1);
    assert!(client_b.as_array().expect("client B list").is_empty());
    assert!(harness.root.join(CLIENT_A).join("consents.json").is_file());
    assert!(harness.root.join(CLIENT_B).join("consents.json").is_file());
}

#[tokio::test]
async fn ledger_endpoint_is_empty_per_client_before_actions() {
    let harness = harness();
    for client in [CLIENT_A, CLIENT_B] {
        let (status, body) = request(
            &harness.app,
            Method::GET,
            &format!("/v1/clients/{client}/ledger"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.as_array().expect("ledger list").is_empty());
    }
}

#[tokio::test]
async fn reset_restores_two_public_summaries_and_clears_consent_state() {
    let harness = harness();
    let (status, created) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        Some(consent_body(None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (status, clients) = request(
        &harness.app,
        Method::POST,
        "/v1/demo/reset",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{clients}");
    let clients = clients.as_array().expect("demo clients");
    assert_eq!(clients.len(), 2);
    assert!(clients.iter().all(|client| client["test_data"] == true));

    let raw_a = pru_ssn::synthetic_test_ssn(1, pru_ssn::TestSsnRange::Group00);
    let raw_b = pru_ssn::synthetic_test_ssn(2, pru_ssn::TestSsnRange::Area9xx);
    let response_text = serde_json::to_string(clients).expect("response text");
    assert!(!response_text.contains(&raw_a));
    assert!(!response_text.contains(&raw_b));

    let (_, listed) = request(
        &harness.app,
        Method::GET,
        &format!("/v1/clients/{CLIENT_A}/consents"),
        None,
    )
    .await;
    assert!(listed.as_array().expect("consent list").is_empty());
}

#[tokio::test]
async fn invalid_client_path_cannot_escape_the_data_root() {
    let harness = harness();
    let (status, body) = request(
        &harness.app,
        Method::POST,
        "/v1/clients/client..evil/consents",
        Some(consent_body(None)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!harness.root.join("client..evil").exists());
}
