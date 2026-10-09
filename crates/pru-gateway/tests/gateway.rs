use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
};
use chrono::{DateTime, TimeZone, Utc};
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord, RevocationList};
use pru_gateway::{
    ActionResponse, Clock, GatewayConfig, GatewayState, TOKEN_FACTORY_RECIPIENT, router,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tower::ServiceExt;

const CLIENT_ID: &str = "synthetic-client";
const PURPOSE: &str = "answer a synthetic tax-practice question";

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct Harness {
    app: Router,
    local_calls: Arc<Mutex<Vec<Value>>>,
    remote_calls: Arc<Mutex<Vec<Value>>>,
    ledger_path: std::path::PathBuf,
    _temp: TempDir,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn stub_upstream() -> (String, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().expect("calls lock").push(body);
                    Json(json!({
                        "id": "synthetic-stub-response",
                        "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                    }))
                },
            ),
        )
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let address = listener.local_addr().expect("stub address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    (format!("http://{address}/v1"), calls, task)
}

async fn harness(kind: ConsentKind, recipient: Option<&str>) -> (Harness, String) {
    let (local_url, local_calls, local_task) = stub_upstream().await;
    let (remote_url, remote_calls, remote_task) = stub_upstream().await;
    let temp = TempDir::new().expect("temp directory");
    let ledger_path = temp.path().join("egress.jsonl");
    let authority = ConsentAuthority::new();
    let signed_at = Utc
        .with_ymd_and_hms(2026, 10, 5, 12, 0, 0)
        .single()
        .expect("fixed timestamp");
    let record = ConsentRecord::new(
        CLIENT_ID,
        kind,
        recipient.map(str::to_owned),
        PURPOSE,
        signed_at,
        "Synthetic Test Signer",
        "12345",
    )
    .expect("consent record");
    let token = authority.mint(&record, "12345").expect("mint token");
    let config = GatewayConfig {
        local_health_url: format!("{}/health", local_url.trim_end_matches("/v1")),
        local_base_url: local_url,
        local_model: "nemotron-3-nano-4b".to_owned(),
        local_api_key: Some("synthetic-local-key".to_owned()),
        token_factory_base_url: remote_url,
        nebius_api_key: "synthetic-nebius-key".to_owned(),
        ledger_path: ledger_path.clone(),
        client_data_root: None,
        ledger_hash_key: [7; 32],
    };
    let state = GatewayState::with_clock(
        authority.verifier(),
        RevocationList::default(),
        config,
        Arc::new(FixedClock(
            Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
                .single()
                .expect("fixed timestamp"),
        )),
    )
    .expect("gateway state");
    (
        Harness {
            app: router(state),
            local_calls,
            remote_calls,
            ledger_path,
            _temp: temp,
            tasks: vec![local_task, remote_task],
        },
        token,
    )
}

async fn call(app: &Router, action: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/actions/{action}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("request json")))
                .expect("request"),
        )
        .await
        .expect("gateway response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("response body");
    (
        status,
        serde_json::from_slice(&body).expect("response json"),
    )
}

fn action_body(token: &str, content: &str) -> Value {
    json!({
        "client_id": CLIENT_ID,
        "consent_token": token,
        "purpose": PURPOSE,
        "arguments": {
            "messages": [{"role": "user", "content": content}],
            "max_tokens": 16
        }
    })
}

fn read_ledger(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read ledger")
}

#[tokio::test]
async fn ssn_bearing_prompt_routes_only_to_local_and_ledger_has_only_hash() {
    let (harness, token) = harness(ConsentKind::Use, None).await;
    let synthetic_ssn = "900-12-3456";
    let (status, body) = call(
        &harness.app,
        "llm_complete",
        action_body(
            &token,
            &format!("Synthetic canary SSN {synthetic_ssn}. Return the format name."),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: ActionResponse = serde_json::from_value(body).expect("action response");
    assert_eq!(response.route, "local");
    assert!(response.contains_ssn);
    assert_eq!(harness.local_calls.lock().expect("local calls").len(), 1);
    assert!(
        harness
            .remote_calls
            .lock()
            .expect("remote calls")
            .is_empty()
    );

    let ledger = read_ledger(&harness.ledger_path);
    assert!(!ledger.contains(synthetic_ssn));
    let entry: Value = serde_json::from_str(ledger.trim()).expect("ledger entry");
    assert_eq!(entry["decision"], "permit");
    assert_eq!(entry["route"], "local");
    assert_eq!(
        entry["span_hashes"].as_array().expect("span hashes").len(),
        1
    );
}

#[tokio::test]
async fn ssn_field_name_supplies_context_for_plain_string_and_number_arguments() {
    for ssn_value in [json!("900121001"), json!(900121001)] {
        let (harness, token) = harness(ConsentKind::Use, None).await;
        let body = json!({
            "client_id": CLIENT_ID,
            "consent_token": token,
            "purpose": PURPOSE,
            "arguments": {"ssn": ssn_value, "instruction": "Synthetic routing test"}
        });
        let (status, body) = call(&harness.app, "llm_complete", body).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let response: ActionResponse = serde_json::from_value(body).expect("action response");
        assert_eq!(response.route, "local");
        assert!(response.contains_ssn);
        assert_eq!(harness.local_calls.lock().expect("local calls").len(), 1);
        assert!(
            harness
                .remote_calls
                .lock()
                .expect("remote calls")
                .is_empty()
        );
        assert!(!read_ledger(&harness.ledger_path).contains("900121001"));
    }
}

#[tokio::test]
async fn ssn_free_prompt_with_disclosure_consent_routes_to_token_factory() {
    let (harness, token) = harness(ConsentKind::Disclose, Some(TOKEN_FACTORY_RECIPIENT)).await;
    let (status, body) = call(
        &harness.app,
        "llm_complete",
        action_body(&token, "This is synthetic. What is 17 plus 23?"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: ActionResponse = serde_json::from_value(body).expect("action response");
    assert_eq!(response.route, "token_factory");
    assert!(!response.contains_ssn);
    assert!(harness.local_calls.lock().expect("local calls").is_empty());
    let calls = harness.remote_calls.lock().expect("remote calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["model"], "nvidia/Nemotron-3_5-Lightning");
    assert_eq!(calls[0]["chat_template_kwargs"]["enable_thinking"], false);
}

#[tokio::test]
async fn wrong_consent_kind_is_denied_before_any_upstream_call() {
    let (harness, token) = harness(ConsentKind::Use, None).await;
    let (status, body) = call(
        &harness.app,
        "llm_complete",
        action_body(&token, "Synthetic and SSN-free request."),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(harness.local_calls.lock().expect("local calls").is_empty());
    assert!(
        harness
            .remote_calls
            .lock()
            .expect("remote calls")
            .is_empty()
    );
    let ledger = read_ledger(&harness.ledger_path);
    let entry: Value = serde_json::from_str(ledger.trim()).expect("ledger entry");
    assert_eq!(entry["decision"], "deny");
}

#[tokio::test]
async fn email_and_calendar_build_requests_but_do_not_send() {
    for action in ["send_email", "create_event"] {
        let recipient = "Synthetic Recipient";
        let (harness, token) = harness(ConsentKind::Disclose, Some(recipient)).await;
        let body = json!({
            "client_id": CLIENT_ID,
            "consent_token": token,
            "purpose": PURPOSE,
            "recipient": recipient,
            "arguments": {"subject": "Synthetic dry run", "body": "No taxpayer data."}
        });
        let (status, body) = call(&harness.app, action, body).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let response: ActionResponse = serde_json::from_value(body).expect("action response");
        assert!(response.dry_run);
        assert_eq!(response.route, "dry_run");
        assert_eq!(response.result.expect("dry-run result")["sent"], false);
        assert!(harness.local_calls.lock().expect("local calls").is_empty());
        assert!(
            harness
                .remote_calls
                .lock()
                .expect("remote calls")
                .is_empty()
        );
    }
}
