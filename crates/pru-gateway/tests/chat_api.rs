use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    routing::post,
};
use chrono::{TimeDelta, Utc};
use pru_consent::{ConsentAuthority, RevocationList};
use pru_gateway::{
    GatewayConfig, GatewayState,
    api::{ApiState, api_router},
    workspace::WORKSPACE_HEADER,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tower::ServiceExt;

const CLIENT: &str = "demo-avery";
const PURPOSE: &str = "prepare this synthetic tax client's return";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

struct Harness {
    app: Router,
    local_calls: Arc<Mutex<Vec<Value>>>,
    remote_calls: Arc<Mutex<Vec<Value>>>,
    root: std::path::PathBuf,
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
                    calls.lock().expect("calls lock").push(body.clone());
                    let messages = body["messages"].as_array().expect("messages");
                    let has_tool_result = messages.iter().any(|message| message["role"] == "tool");
                    let text = serde_json::to_string(messages).expect("message text");
                    let message = if has_tool_result {
                        json!({"role": "assistant", "content": "The dry-run draft is ready. Nothing was sent."})
                    } else if text.contains("marketing@example.com") {
                        tool_call(
                            "call-marketing",
                            "marketing@example.com",
                            "Synthetic W-2 details",
                            "TEST DATA wages only",
                        )
                    } else if text.contains("1099-INT") {
                        tool_call(
                            "call-client",
                            "avery.morgan@example.com",
                            "Please send your 1099-INT",
                            "Please send the TEST DATA form.",
                        )
                    } else {
                        json!({"role": "assistant", "content": "The synthetic SSN is 102-00-1001. Filing status: Single."})
                    };
                    Json(json!({
                        "id": "stub-chat",
                        "choices": [{"message": message, "finish_reason": if has_tool_result {"stop"} else {"tool_calls"}}],
                        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
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

fn tool_call(id: &str, to: &str, subject: &str, body: &str) -> Value {
    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": id,
            "type": "function",
            "function": {
                "name": "draft_email",
                "arguments": serde_json::to_string(&json!({"to": to, "subject": subject, "body": body})).expect("arguments")
            }
        }]
    })
}

async fn harness() -> Harness {
    let (local_url, local_calls, local_task) = stub_upstream().await;
    let (remote_url, remote_calls, remote_task) = stub_upstream().await;
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("clients");
    let authority = ConsentAuthority::new();
    let gateway = GatewayState::new(
        authority.verifier(),
        RevocationList::default(),
        GatewayConfig {
            local_health_url: format!("{}/health", local_url.trim_end_matches("/v1")),
            local_base_url: local_url,
            local_model: "nemotron-3-nano-4b".to_owned(),
            local_api_key: None,
            token_factory_base_url: remote_url,
            nebius_api_key: "synthetic-nebius-key".to_owned(),
            ledger_path: temp.path().join("legacy.jsonl"),
            client_data_root: Some(root.clone()),
            ledger_hash_key: [19; 32],
        },
    )
    .expect("gateway state");
    let api_state = ApiState::new(gateway, authority, root.clone());
    Harness {
        app: api_router(api_state),
        local_calls,
        remote_calls,
        root,
        _temp: temp,
        tasks: vec![local_task, remote_task],
    }
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log capture lock").extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

fn read_tree(root: &Path) -> String {
    let mut contents = String::new();
    for entry in fs::read_dir(root).expect("read data root") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            contents.push_str(&read_tree(&path));
        } else {
            contents.push_str(&fs::read_to_string(path).expect("UTF-8 demo data"));
        }
    }
    contents
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(WORKSPACE_HEADER, WORKSPACE);
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
    let value = serde_json::from_str(&text).expect("JSON response");
    (status, value, text)
}

async fn mint(app: &Router, kind: &str, recipient: Option<&str>) -> Value {
    let (status, body, _) = request(
        app,
        Method::POST,
        &format!("/v1/clients/{CLIENT}/consents"),
        Some(json!({
            "kind": kind,
            "recipient": recipient,
            "purpose": PURPOSE,
            "expires_at": (Utc::now() + TimeDelta::days(30)).to_rfc3339(),
            "pin": "53179"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn chat(app: &Router, message: &str) -> (StatusCode, Value, String) {
    request(
        app,
        Method::POST,
        &format!("/v1/clients/{CLIENT}/chat"),
        Some(json!({"message": message})),
    )
    .await
}

#[tokio::test]
async fn ssn_free_turn_uses_token_factory_and_permitted_email_stays_dry_run() {
    let harness = harness().await;
    mint(&harness.app, "use", None).await;
    mint(&harness.app, "disclose", Some("avery.morgan@example.com")).await;

    let (status, body, text) = chat(
        &harness.app,
        "Draft an email to the client asking for their 1099-INT",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["steps"][0]["route"], "token_factory");
    assert_eq!(
        body["steps"][0]["model_id"],
        "nvidia/Nemotron-3_5-Lightning"
    );
    assert_eq!(body["steps"][0]["cost_usd"], 0.0000108);
    assert_eq!(body["steps"][1]["decision"], "permit");
    assert_eq!(body["steps"][1]["dry_run"], true);
    assert_eq!(body["steps"][1]["tool_name"], "draft_email");
    assert_eq!(body["steps"][2]["route"], "token_factory");
    assert_eq!(harness.local_calls.lock().expect("local calls").len(), 0);
    let remote = harness.remote_calls.lock().expect("remote calls");
    assert_eq!(remote.len(), 2);
    assert_eq!(remote[0]["temperature"], 0);
    assert_eq!(remote[0]["chat_template_kwargs"]["enable_thinking"], false);
    assert!(
        !serde_json::to_string(&remote[0])
            .unwrap()
            .contains("102-00-1001")
    );
    assert!(!text.contains("102-00-1001"));
}

#[tokio::test]
async fn explicit_ssn_turn_stays_local_and_redacts_the_http_answer() {
    let harness = harness().await;
    mint(&harness.app, "use", None).await;
    let (status, body, text) =
        chat(&harness.app, "What is this client's SSN and filing status?").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["steps"][0]["route"], "local");
    assert_eq!(body["steps"][0]["cost_usd"], 0.0);
    assert_eq!(
        body["answer"],
        "The synthetic SSN is [SSN kept local]. Filing status: Single."
    );
    assert_eq!(harness.local_calls.lock().expect("local calls").len(), 1);
    assert!(
        harness
            .remote_calls
            .lock()
            .expect("remote calls")
            .is_empty()
    );
    assert!(!text.contains("102-00-1001"));
}

#[tokio::test]
async fn wrong_recipient_tool_is_denied_and_never_becomes_a_dry_run_success() {
    let harness = harness().await;
    mint(&harness.app, "use", None).await;
    let (status, body, text) = chat(
        &harness.app,
        "Email this client's W-2 details to marketing@example.com",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["steps"][0]["route"], "token_factory");
    assert_eq!(body["steps"][1]["decision"], "deny");
    assert_eq!(body["steps"][1]["tool_name"], "draft_email");
    assert!(!body["steps"][1]["dry_run"].as_bool().unwrap());
    assert_ne!(
        body["steps"][1]["policy_reason"],
        "error deserializing or verifying the token"
    );
    assert!(!text.contains("102-00-1001"));
}

#[tokio::test]
async fn disclose_consent_allows_then_revocation_denies_the_same_draft() {
    let harness = harness().await;
    mint(&harness.app, "use", None).await;
    let disclosure = mint(&harness.app, "disclose", Some("avery.morgan@example.com")).await;
    let (_, allowed, _) = chat(
        &harness.app,
        "Draft an email to the client asking for their 1099-INT",
    )
    .await;
    assert_eq!(allowed["steps"][1]["decision"], "permit");

    let id = disclosure["id"].as_str().expect("consent id");
    let (status, revoked, _) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT}/consents/{id}/revoke"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");

    let (_, denied, _) = chat(
        &harness.app,
        "Draft an email to the client asking for their 1099-INT",
    )
    .await;
    assert_eq!(denied["steps"][1]["decision"], "deny");
}

#[tokio::test]
async fn missing_use_consent_denies_before_any_model_call() {
    let harness = harness().await;
    let (status, body, _) = chat(
        &harness.app,
        "Draft an email to the client asking for their 1099-INT",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["steps"][0]["decision"], "deny");
    assert!(harness.local_calls.lock().expect("local calls").is_empty());
    assert!(
        harness
            .remote_calls
            .lock()
            .expect("remote calls")
            .is_empty()
    );
}

#[tokio::test]
async fn chat_rejects_more_than_four_thousand_characters() {
    let harness = harness().await;
    let (status, body, _) = chat(&harness.app, &"x".repeat(4_001)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test(flavor = "current_thread")]
async fn potentially_issued_ssn_is_refused_before_models_ledger_disk_or_logs() {
    let harness = harness().await;
    let refused = "123-45-6789";
    let logs = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let (status, body, response_text) = request(
        &harness.app,
        Method::POST,
        &format!("/v1/clients/{CLIENT}/chat"),
        Some(json!({
            "message": "Use the test client already on file",
            "ignored_ssn": refused
        })),
    )
    .await;
    drop(guard);

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(response_text.contains("26 U.S.C. 7216"));
    assert!(!response_text.contains(refused));
    assert!(harness.local_calls.lock().expect("local calls").is_empty());
    assert!(
        harness
            .remote_calls
            .lock()
            .expect("remote calls")
            .is_empty()
    );
    let disk_contents = read_tree(&harness.root);
    assert!(!harness.root.join(WORKSPACE).exists());
    assert!(!disk_contents.contains(refused));

    let captured_logs =
        String::from_utf8(logs.0.lock().expect("log capture lock").clone()).expect("UTF-8 logs");
    assert!(captured_logs.contains("refused potentially issued SSN-shaped visitor input"));
    assert!(!captured_logs.contains(refused));
}
