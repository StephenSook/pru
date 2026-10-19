use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    routing::post,
};
use chrono::{TimeDelta, Utc};
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord, RevocationList};
use pru_gateway::{
    GatewayConfig, GatewayState,
    agent::{AgentCredential, AgentTurnLease},
    api::{ApiState, api_router},
    mcp::{MCP_TOOL_NAMES, mcp_router},
    workspace::{WORKSPACE_HEADER, WorkspaceManager},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tower::ServiceExt;

const CLIENT: &str = "demo-avery";
const PURPOSE: &str = "prepare this synthetic tax client's return";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const SECRET: &str = "3333333333333333333333333333333333333333333333333333333333333333";

struct Harness {
    mcp: Router,
    api: Router,
    state: ApiState,
    root: std::path::PathBuf,
    local_calls: Arc<Mutex<Vec<Value>>>,
    turn: AgentTurnLease,
    _temp: TempDir,
    task: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    fn seed_pending(&self, name: &str, arguments: Value) {
        self.state
            .agent_turns()
            .replace_pending_tool_calls(
                self.turn.turn_id(),
                &json!({
                    "choices": [{"message": {"tool_calls": [{
                        "id": "call-test",
                        "type": "function",
                        "function": {
                            "name": format!("mcp__pru__{name}"),
                            "arguments": serde_json::to_string(&arguments).expect("arguments")
                        }
                    }]}}]
                }),
            )
            .expect("pending tool call");
    }

    fn ledger(&self) -> Vec<Value> {
        let path = self.root.join(WORKSPACE).join(CLIENT).join("ledger.jsonl");
        std::fs::read_to_string(path)
            .expect("ledger")
            .lines()
            .map(|line| serde_json::from_str(line).expect("ledger row"))
            .collect()
    }

    fn notes(&self) -> String {
        std::fs::read_to_string(self.root.join(WORKSPACE).join(CLIENT).join("notes.jsonl"))
            .expect("notes")
    }
}

async fn local_stub() -> (String, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().expect("calls lock").push(body);
                    Json(json!({
                        "id": "local-stub",
                        "choices": [{"message": {
                            "role": "assistant",
                            "content": "Local confirmation for synthetic SSN 900-12-3456."
                        }}],
                        "usage": {"prompt_tokens": 30, "completion_tokens": 10, "total_tokens": 40}
                    }))
                },
            ),
        )
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local stub");
    let address = listener.local_addr().expect("local stub address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve local stub");
    });
    (format!("http://{address}/v1"), calls, task)
}

async fn harness() -> Harness {
    let (local_url, local_calls, task) = local_stub().await;
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("clients");
    let authority = ConsentAuthority::new();
    let use_record = ConsentRecord::new(
        CLIENT,
        ConsentKind::Use,
        None,
        PURPOSE,
        Utc::now(),
        "Synthetic Tester",
        "53179",
    )
    .expect("use consent");
    let use_token = authority.mint(&use_record, "53179").expect("use token");
    let gateway = GatewayState::new(
        authority.verifier(),
        RevocationList::default(),
        GatewayConfig {
            local_health_url: format!("{}/health", local_url.trim_end_matches("/v1")),
            local_base_url: local_url,
            local_model: "nemotron-3-nano-4b".to_owned(),
            local_api_key: None,
            token_factory_base_url: "http://127.0.0.1:9/v1".to_owned(),
            nebius_api_key: Some("synthetic-nebius-key".to_owned()),
            ledger_path: temp.path().join("legacy.jsonl"),
            client_data_root: Some(root.clone()),
            ledger_hash_key: [37; 32],
        },
    )
    .expect("gateway");
    let state = ApiState::with_agent_credential(
        gateway,
        authority,
        root.clone(),
        AgentCredential::from_hex(SECRET).expect("credential"),
    );
    let manager = WorkspaceManager::new(root.clone());
    let (workspace, _) = manager.acquire(WORKSPACE, false).await.expect("workspace");
    state.reset_demo(&workspace).expect("reset demo");
    let turn = state
        .agent_turns()
        .begin(workspace, CLIENT.to_owned(), PURPOSE.to_owned(), use_token)
        .await
        .expect("agent turn");
    Harness {
        mcp: mcp_router(state.clone()),
        api: api_router(state.clone()),
        state,
        root,
        local_calls,
        turn,
        _temp: temp,
        task,
    }
}

async fn raw_request(app: &Router, method: Method, body: Option<Value>) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri("/mcp")
        .header("host", "127.0.0.1:8787")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25");
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
        .expect("response body")
        .to_vec();
    (status, bytes)
}

async fn rpc(app: &Router, id: u64, method: &str, params: Value) -> Value {
    let (status, body) = raw_request(
        app,
        Method::POST,
        Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("JSON-RPC response")
}

async fn call_tool(app: &Router, name: &str, arguments: Value) -> Value {
    rpc(
        app,
        7,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
    .await
}

async fn mint_disclosure(api: &Router, recipient: &str) {
    let response = api
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/clients/{CLIENT}/consents"))
                .header(WORKSPACE_HEADER, WORKSPACE)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "kind": "disclose",
                        "recipient": recipient,
                        "purpose": PURPOSE,
                        "expires_at": (Utc::now() + TimeDelta::days(30)).to_rfc3339(),
                        "pin": "53179"
                    }))
                    .expect("consent request"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn tools_list_names_exactly_the_seven_pru_tools() {
    let harness = harness().await;
    let body = rpc(&harness.mcp, 1, "tools/list", json!({})).await;
    let names = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert_eq!(names, MCP_TOOL_NAMES);
}

#[tokio::test]
async fn measured_hermes_initialize_and_initialized_notification_are_accepted() {
    let harness = harness().await;
    let initialized = rpc(
        &harness.mcp,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "hermes-agent", "version": "0.22.2"}
        }),
    )
    .await;
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");

    let (status, _) = raw_request(
        &harness.mcp,
        Method::POST,
        Some(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn unmatched_tool_call_is_refused_without_a_row_or_side_effect() {
    let harness = harness().await;
    let body = call_tool(&harness.mcp, "remember_note", json!({"text": "unmatched"})).await;
    assert_eq!(body["result"]["isError"], true);
    assert!(harness.ledger().is_empty());
    assert!(harness.notes().is_empty());
}

#[tokio::test]
async fn draft_requires_exact_disclosure_consent_and_never_sends() {
    let harness = harness().await;
    let arguments = json!({
        "to": "avery.morgan@example.com",
        "subject": "Synthetic follow-up",
        "body": "Please send the TEST DATA form."
    });
    harness.seed_pending("draft_client_email", arguments.clone());
    let denied = call_tool(&harness.mcp, "draft_client_email", arguments.clone()).await;
    assert_eq!(denied["result"]["isError"], true);
    assert!(
        denied["result"]["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("consent_facts_do_not_match")
    );

    mint_disclosure(&harness.api, "avery.morgan@example.com").await;
    harness.seed_pending("draft_client_email", arguments.clone());
    let allowed = call_tool(&harness.mcp, "draft_client_email", arguments).await;
    assert_eq!(allowed["result"]["isError"], false);
    assert_eq!(allowed["result"]["structuredContent"]["sent"], false);
}

#[tokio::test]
async fn summary_redacts_the_ssn_and_ledgers_the_redacted_span() {
    let harness = harness().await;
    harness.seed_pending("get_client_summary", json!({}));
    let body = call_tool(&harness.mcp, "get_client_summary", json!({})).await;
    assert_eq!(body["result"]["isError"], false);
    let encoded = serde_json::to_string(&body["result"]).expect("result JSON");
    assert!(pru_ssn::recognize(&encoded).is_empty());
    assert!(encoded.contains("[SSN kept local]"));
    let ledger = harness.ledger();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0]["contains_ssn"], false);
    assert_eq!(ledger[0]["redacted_spans"], 1);
    assert_eq!(
        ledger[0]["span_hashes"].as_array().expect("hashes").len(),
        1
    );
    assert_eq!(ledger[0]["tool"], "get_client_summary");
}

#[tokio::test]
async fn ssn_bearing_note_is_denied_without_writing_notes() {
    let harness = harness().await;
    let arguments = json!({"text": "Remember synthetic SSN 900-12-3456"});
    harness.seed_pending("remember_note", arguments.clone());
    let body = call_tool(&harness.mcp, "remember_note", arguments).await;
    assert_eq!(body["result"]["isError"], true);
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("ssn_requires_local_destination")
    );
    assert!(harness.notes().is_empty());
    assert_eq!(harness.ledger().len(), 1);
}

#[tokio::test]
async fn twenty_first_note_is_refused_without_growing_the_file() {
    let harness = harness().await;
    let path = harness
        .root
        .join(WORKSPACE)
        .join(CLIENT)
        .join("notes.jsonl");
    let body = (0..20)
        .map(|index| {
            json!({
                "id": format!("00000000-0000-4000-8000-{index:012}"),
                "text": format!("Existing note {index}"),
                "created_at": "2026-10-19T12:00:00Z",
                "ledger_entry_id": format!("10000000-0000-4000-8000-{index:012}")
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{body}\n")).expect("write twenty notes");

    let arguments = json!({"text": "One note too many"});
    harness.seed_pending("remember_note", arguments.clone());
    let response = call_tool(&harness.mcp, "remember_note", arguments).await;

    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("maximum 20 notes")
    );
    assert_eq!(harness.notes().lines().count(), 20);
}

#[tokio::test]
async fn local_model_answer_is_redacted_before_returning_to_hermes() {
    let harness = harness().await;
    let arguments = json!({"question": "Check this client's SSN"});
    harness.seed_pending("ask_local_model", arguments.clone());
    let body = call_tool(&harness.mcp, "ask_local_model", arguments).await;
    assert_eq!(body["result"]["isError"], false, "{body}");
    let encoded = serde_json::to_string(&body["result"]).expect("result JSON");
    assert!(pru_ssn::recognize(&encoded).is_empty());
    assert!(encoded.contains("[SSN kept local]"));
    assert_eq!(harness.local_calls.lock().expect("local calls").len(), 1);
    let ledger = harness.ledger();
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0]["route"], "local");
    assert_eq!(ledger[0]["contains_ssn"], true);
    assert_eq!(ledger[1]["route"], "dynamic");
    assert_eq!(ledger[1]["redacted_spans"], 1);
}

#[tokio::test]
async fn head_and_get_answer_without_hanging() {
    let harness = harness().await;
    for method in [Method::HEAD, Method::GET] {
        let (status, _) = raw_request(&harness.mcp, method, None).await;
        assert!(!status.is_server_error(), "{status}");
    }
}
