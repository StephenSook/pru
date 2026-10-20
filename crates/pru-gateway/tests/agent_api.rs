use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    routing::post,
};
use chrono::Utc;
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord, RevocationList};
use pru_gateway::{
    GatewayConfig, GatewayState, TOKEN_FACTORY_MODEL,
    agent::{AgentCredential, AgentTurnError, AgentTurnLease},
    agent_api::agent_router,
    api::ApiState,
    workspace::WorkspaceManager,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT: &str = "demo-avery";
const PURPOSE: &str = "prepare this synthetic tax client's return";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

struct Harness {
    app: Router,
    state: ApiState,
    calls: Arc<Mutex<Vec<Value>>>,
    root: std::path::PathBuf,
    use_token: String,
    _temp: TempDir,
    task: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    async fn begin(&self, consent_token: String) -> AgentTurnLease {
        let manager = WorkspaceManager::new(self.root.clone());
        let (workspace, _) = manager.acquire(WORKSPACE, false).await.expect("workspace");
        self.state
            .agent_turns()
            .begin(
                workspace,
                CLIENT.to_owned(),
                PURPOSE.to_owned(),
                consent_token,
            )
            .await
            .expect("agent turn")
    }

    fn ledger(&self) -> Vec<Value> {
        let path = self.root.join(WORKSPACE).join(CLIENT).join("ledger.jsonl");
        match std::fs::read_to_string(path) {
            Ok(contents) => contents
                .lines()
                .map(|line| serde_json::from_str(line).expect("ledger row"))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read ledger: {error}"),
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
                        "id": "stub-agent",
                        "object": "chat.completion",
                        "choices": [{
                            "index": 0,
                            "finish_reason": "tool_calls",
                            "message": {
                                "role": "assistant",
                                "content": null,
                                "tool_calls": [{
                                    "id": "call-summary",
                                    "type": "function",
                                    "function": {
                                        "name": "mcp__pru__get_client_summary",
                                        "arguments": "{\"b\":2,\"a\":1}"
                                    }
                                }]
                            }
                        }],
                        "usage": {
                            "prompt_tokens": 100,
                            "completion_tokens": 20,
                            "total_tokens": 120
                        }
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

async fn harness() -> Harness {
    let (upstream, calls, task) = stub_upstream().await;
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("clients");
    let authority = ConsentAuthority::new();
    let record = ConsentRecord::new(
        CLIENT,
        ConsentKind::Use,
        None,
        PURPOSE,
        Utc::now(),
        "Synthetic Tester",
        "53179",
    )
    .expect("consent record");
    let use_token = authority.mint(&record, "53179").expect("use token");
    let gateway = GatewayState::new(
        authority.verifier(),
        RevocationList::default(),
        GatewayConfig {
            local_health_url: "http://127.0.0.1:9/health".to_owned(),
            local_base_url: "http://127.0.0.1:9/v1".to_owned(),
            local_model: "nemotron-3-nano-4b".to_owned(),
            local_api_key: None,
            token_factory_base_url: upstream,
            nebius_api_key: Some("synthetic-nebius-key".to_owned()),
            ledger_path: temp.path().join("legacy.jsonl"),
            client_data_root: Some(root.clone()),
            ledger_hash_key: [31; 32],
        },
    )
    .expect("gateway");
    let state = ApiState::with_agent_credential(
        gateway,
        authority,
        root.clone(),
        AgentCredential::from_hex(SECRET).expect("credential"),
    );
    Harness {
        app: agent_router(state.clone()),
        state,
        calls,
        root,
        use_token,
        _temp: temp,
        task,
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    turn_id: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    if let Some(turn_id) = turn_id {
        builder = builder.header("x-pru-turn", turn_id.to_string());
    }
    let bytes = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        serde_json::to_vec(&body).expect("request JSON")
    } else {
        Vec::new()
    };
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(bytes)).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000)
        .await
        .expect("response body");
    let value = serde_json::from_slice(&bytes).expect("JSON response");
    (status, value)
}

fn model_body(content: &str) -> Value {
    json!({
        "messages": [{"role": "user", "content": content}],
        "tools": [{"type": "function", "function": {"name": "mcp__pru__get_client_summary"}}],
        "tool_choice": "auto",
        "max_tokens": 900,
        "temperature": 0.7,
        "reasoning_effort": "medium",
        "stream": true
    })
}

#[tokio::test]
async fn ssn_context_is_denied_before_upstream_contact() {
    let harness = harness().await;
    let turn = harness.begin(harness.use_token.clone()).await;
    let (status, body) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(turn.turn_id()),
        Some(model_body("Use synthetic SSN 900-12-3456")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("ssn_requires_local_destination")
    );
    assert!(harness.calls.lock().expect("calls lock").is_empty());
    let ledger = harness.ledger();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["policy_reason"], "ssn_requires_local_destination");
    assert_eq!(ledger[0]["route"], "token_factory");
}

#[tokio::test]
async fn empty_use_consent_is_refused_with_the_agent_reason() {
    let harness = harness().await;
    let turn = harness.begin(String::new()).await;
    let (status, body) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(turn.turn_id()),
        Some(model_body("Summarize this client")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(harness.calls.lock().expect("calls lock").is_empty());
    let ledger = harness.ledger();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["policy_reason"], "matching_consent_required");
    assert!(matches!(
        harness
            .state
            .agent_turns()
            .reserve_model_call(turn.turn_id()),
        Err(AgentTurnError::Terminated)
    ));
}

#[tokio::test]
async fn bad_bearer_is_unauthorized_without_a_ledger_row() {
    let harness = harness().await;
    let turn = harness.begin(harness.use_token.clone()).await;
    let (status, _) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(&"22".repeat(32)),
        Some(turn.turn_id()),
        Some(model_body("Summarize this client")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(harness.ledger().is_empty());
    assert!(harness.calls.lock().expect("calls lock").is_empty());
}

#[tokio::test]
async fn missing_or_mismatched_active_turn_is_a_conflict() {
    let harness = harness().await;
    let (status, _) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(Uuid::new_v4()),
        Some(model_body("Summarize this client")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let turn = harness.begin(harness.use_token.clone()).await;
    let (status, _) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(Uuid::new_v4()),
        Some(model_body("Summarize this client")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_ne!(turn.turn_id(), Uuid::nil());
    assert!(harness.calls.lock().expect("calls lock").is_empty());
    assert!(harness.ledger().is_empty());
}

#[tokio::test]
async fn seventh_model_call_is_refused_without_upstream_contact() {
    let harness = harness().await;
    let turn = harness.begin(harness.use_token.clone()).await;
    for _ in 0..6 {
        let (status, _) = request(
            &harness.app,
            Method::POST,
            "/agent/v1/chat/completions",
            Some(SECRET),
            Some(turn.turn_id()),
            Some(model_body("Summarize this client")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(turn.turn_id()),
        Some(model_body("One more")),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(harness.calls.lock().expect("calls lock").len(), 6);
}

#[tokio::test]
async fn body_is_normalized_budget_reconciles_and_tool_call_becomes_pending() {
    let harness = harness().await;
    let turn = harness.begin(harness.use_token.clone()).await;
    let (status, body) = request(
        &harness.app,
        Method::POST,
        "/agent/v1/chat/completions",
        Some(SECRET),
        Some(turn.turn_id()),
        Some(model_body("Summarize this client")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let calls = harness.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    let sent = &calls[0];
    assert_eq!(sent["model"], TOKEN_FACTORY_MODEL);
    assert_eq!(sent["max_tokens"], 600);
    assert_eq!(sent["temperature"], 0);
    assert_eq!(sent["chat_template_kwargs"]["enable_thinking"], false);
    assert!(sent.get("reasoning_effort").is_none());
    assert!(sent.get("stream").is_none());
    drop(calls);

    let budget: Value = serde_json::from_slice(
        &std::fs::read(harness.root.join(".instance/token-budget.json")).expect("budget file"),
    )
    .expect("budget JSON");
    assert_eq!(budget["charged_tokens"], 120);

    let active = harness
        .state
        .agent_turns()
        .active(turn.turn_id())
        .expect("active turn");
    assert_eq!(active.pending_tool_calls.len(), 1);
    assert_eq!(active.pending_tool_calls[0].name, "get_client_summary");
    assert_eq!(
        active.pending_tool_calls[0].canonical_arguments,
        r#"{"a":1,"b":2}"#
    );

    let ledger = harness.ledger();
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0]["decision"], "permit");
    assert_eq!(ledger[0]["actor"], "hermes");
    assert_eq!(ledger[1]["event"], "completion");
    assert_eq!(ledger[1]["input_tokens"], 100);
    assert_eq!(ledger[1]["output_tokens"], 20);
    assert_eq!(ledger[1]["authorization_id"], ledger[0]["id"]);
}

#[tokio::test]
async fn models_lists_only_the_selected_token_factory_model() {
    let harness = harness().await;
    let (status, body) = request(
        &harness.app,
        Method::GET,
        "/agent/v1/models",
        Some(SECRET),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"].as_array().expect("models").len(), 1);
    assert_eq!(body["data"][0]["id"], TOKEN_FACTORY_MODEL);
    assert_eq!(body["data"][0]["context_length"], 131_072);
}
