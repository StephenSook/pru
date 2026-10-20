use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use chrono::{TimeDelta, Utc};
use pru_consent::{ConsentAuthority, RevocationList};
use pru_gateway::{
    GatewayConfig, GatewayState,
    agent::AgentCredential,
    agent_api::agent_router,
    api::{ApiState, api_router},
    chat::{AgentBackend, HERMES_AGENT_VERSION, HermesAgentConfig},
    mcp::mcp_router,
    workspace::WORKSPACE_HEADER,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;

const CLIENT: &str = "demo-avery";
const PURPOSE: &str = "prepare this synthetic tax client's return";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const AGENT_SECRET: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const HERMES_API_KEY: &str = "synthetic-hermes-api-key";

#[derive(Clone)]
struct FakeHermesState {
    gateway_url: String,
    model_calls: Arc<Mutex<Vec<Value>>>,
    mcp_calls: Arc<Mutex<usize>>,
    received: Arc<Mutex<Vec<Value>>>,
    deleted: Arc<Mutex<Vec<String>>>,
    delete_conflicts_remaining: Arc<Mutex<usize>>,
}

struct Harness {
    url: String,
    token_factory_calls: Arc<Mutex<Vec<Value>>>,
    hermes: FakeHermesState,
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

async fn token_factory_stub() -> (String, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    let mut calls = calls.lock().expect("token factory calls lock");
                    calls.push(body);
                    let message = if calls.len() == 1 {
                        json!({
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call-summary",
                                "type": "function",
                                "function": {
                                    "name": "mcp__pru__get_client_summary",
                                    "arguments": "{}"
                                }
                            }]
                        })
                    } else {
                        json!({
                            "role": "assistant",
                            "content": "The TEST DATA client is Avery Morgan."
                        })
                    };
                    Json(json!({
                        "id": format!("token-factory-{}", calls.len()),
                        "choices": [{"message": message}],
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
        .expect("bind token factory stub");
    let address = listener.local_addr().expect("token factory address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve token factory stub");
    });
    (format!("http://{address}/v1"), calls, task)
}

async fn fake_hermes_chat(
    State(state): State<FakeHermesState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {HERMES_API_KEY}"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let turn_id = match headers
        .get("x-hermes-session-id")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => value.to_owned(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    state
        .received
        .lock()
        .expect("Hermes received lock")
        .push(body.clone());
    let client = reqwest::Client::new();
    let first = client
        .post(format!("{}/agent/v1/chat/completions", state.gateway_url))
        .bearer_auth(AGENT_SECRET)
        .header("x-pru-turn", &turn_id)
        .json(&json!({
            "messages": body["messages"],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "mcp__pru__get_client_summary",
                    "description": "Read the selected TEST DATA client summary",
                    "parameters": {"type": "object", "properties": {}}
                }
            }],
            "tool_choice": "auto",
            "reasoning_effort": "medium",
            "stream": false
        }))
        .send()
        .await
        .expect("fake Hermes first model call");
    if !first.status().is_success() {
        return Json(json!({
            "choices": [{"message": {
                "role": "assistant",
                "content": format!("custom rejected your API key with HTTP {}", first.status())
            }}]
        }))
        .into_response();
    }
    let first_body: Value = first.json().await.expect("first model response");
    state
        .model_calls
        .lock()
        .expect("Hermes model calls lock")
        .push(first_body.clone());
    let call = &first_body["choices"][0]["message"]["tool_calls"][0];
    let name = call["function"]["name"]
        .as_str()
        .expect("tool name")
        .strip_prefix("mcp__pru__")
        .expect("Pru MCP prefix");
    let arguments: Value = serde_json::from_str(
        call["function"]["arguments"]
            .as_str()
            .expect("tool arguments"),
    )
    .expect("tool arguments JSON");
    let tool = client
        .post(format!("{}/mcp", state.gateway_url))
        .bearer_auth(AGENT_SECRET)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }))
        .send()
        .await
        .expect("fake Hermes MCP call");
    assert_eq!(tool.status(), StatusCode::OK);
    *state.mcp_calls.lock().expect("MCP calls lock") += 1;
    let tool_body: Value = tool.json().await.expect("MCP response");
    let second = client
        .post(format!("{}/agent/v1/chat/completions", state.gateway_url))
        .bearer_auth(AGENT_SECRET)
        .header("x-pru-turn", &turn_id)
        .json(&json!({
            "messages": [
                body["messages"][0].clone(),
                body["messages"][1].clone(),
                first_body["choices"][0]["message"].clone(),
                {
                    "role": "tool",
                    "tool_call_id": call["id"],
                    "content": serde_json::to_string(&tool_body).expect("tool response JSON")
                }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "mcp__pru__get_client_summary",
                    "description": "Read the selected TEST DATA client summary",
                    "parameters": {"type": "object", "properties": {}}
                }
            }],
            "tool_choice": "auto"
        }))
        .send()
        .await
        .expect("fake Hermes second model call");
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: Value = second.json().await.expect("second model response");
    state
        .model_calls
        .lock()
        .expect("Hermes model calls lock")
        .push(second_body.clone());
    Json(second_body).into_response()
}

async fn delete_fake_session(
    State(state): State<FakeHermesState>,
    AxumPath(session): AxumPath<String>,
    headers: HeaderMap,
) -> StatusCode {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {HERMES_API_KEY}"))
    {
        return StatusCode::UNAUTHORIZED;
    }
    let mut conflicts_remaining = state
        .delete_conflicts_remaining
        .lock()
        .expect("delete conflict counter lock");
    if *conflicts_remaining > 0 {
        *conflicts_remaining -= 1;
        return StatusCode::CONFLICT;
    }
    drop(conflicts_remaining);
    state
        .deleted
        .lock()
        .expect("deleted sessions lock")
        .push(session);
    StatusCode::OK
}

async fn harness() -> Harness {
    let (token_factory_url, token_factory_calls, token_task) = token_factory_stub().await;
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gateway");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    let hermes_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Hermes");
    let hermes_address = hermes_listener.local_addr().expect("fake Hermes address");
    let temp = TempDir::new().expect("temp directory");
    let root = temp.path().join("clients");
    let authority = ConsentAuthority::new();
    let gateway = GatewayState::new(
        authority.verifier(),
        RevocationList::default(),
        GatewayConfig {
            local_base_url: "http://127.0.0.1:1/v1".to_owned(),
            local_health_url: "http://127.0.0.1:1/health".to_owned(),
            local_model: "nemotron-3-nano-4b".to_owned(),
            local_api_key: None,
            token_factory_base_url: token_factory_url,
            token_factory_model: Default::default(),
            nebius_api_key: Some("synthetic-nebius-key".to_owned()),
            ledger_path: temp.path().join("legacy.jsonl"),
            client_data_root: Some(root.clone()),
            ledger_hash_key: [41; 32],
        },
    )
    .expect("gateway state");
    let api_state = ApiState::with_agent_credential(
        gateway.clone(),
        authority,
        root,
        AgentCredential::from_hex(AGENT_SECRET).expect("agent credential"),
    )
    .with_agent_backend(AgentBackend::Hermes(HermesAgentConfig::new(
        format!("http://{hermes_address}"),
        HERMES_API_KEY,
    )));
    let app = api_router(api_state.clone())
        .merge(agent_router(api_state.clone()))
        .merge(mcp_router(api_state));
    let gateway_task = tokio::spawn(async move {
        axum::serve(gateway_listener, app)
            .await
            .expect("serve gateway");
    });
    let hermes = FakeHermesState {
        gateway_url: format!("http://{gateway_address}"),
        model_calls: Arc::new(Mutex::new(Vec::new())),
        mcp_calls: Arc::new(Mutex::new(0)),
        received: Arc::new(Mutex::new(Vec::new())),
        deleted: Arc::new(Mutex::new(Vec::new())),
        delete_conflicts_remaining: Arc::new(Mutex::new(0)),
    };
    let fake = Router::new()
        .route("/v1/chat/completions", post(fake_hermes_chat))
        .route("/api/sessions/{session}", delete(delete_fake_session))
        .with_state(hermes.clone());
    let hermes_task = tokio::spawn(async move {
        axum::serve(hermes_listener, fake)
            .await
            .expect("serve fake Hermes");
    });
    Harness {
        url: format!("http://{gateway_address}"),
        token_factory_calls,
        hermes,
        _temp: temp,
        tasks: vec![token_task, gateway_task, hermes_task],
    }
}

async fn mint_use(harness: &Harness) {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/clients/{CLIENT}/consents", harness.url))
        .header(WORKSPACE_HEADER, WORKSPACE)
        .json(&json!({
            "kind": "use",
            "recipient": null,
            "purpose": PURPOSE,
            "expires_at": (Utc::now() + TimeDelta::days(30)).to_rfc3339(),
            "pin": "53179"
        }))
        .send()
        .await
        .expect("mint use consent");
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn chat(harness: &Harness, message: &str) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/clients/{CLIENT}/chat", harness.url))
        .header(WORKSPACE_HEADER, WORKSPACE)
        .json(&json!({"message": message}))
        .send()
        .await
        .expect("chat request");
    let status = response.status();
    let body = response.json().await.expect("chat response JSON");
    (status, body)
}

#[tokio::test]
async fn fake_hermes_runs_model_tool_model_and_deletes_the_session() {
    let harness = harness().await;
    mint_use(&harness).await;

    let (status, body) = chat(&harness, "Summarize the selected TEST DATA client").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agent"]["name"], "Hermes Agent");
    assert_eq!(body["agent"]["version"], HERMES_AGENT_VERSION);
    assert_eq!(body["answer"], "The TEST DATA client is Avery Morgan.");
    assert_eq!(body["steps"].as_array().expect("steps").len(), 3);
    assert_eq!(body["steps"][0]["route"], "token_factory");
    assert_eq!(body["steps"][0]["input_tokens"], 100);
    assert_eq!(body["steps"][1]["tool_name"], "get_client_summary");
    assert_eq!(body["steps"][1]["redacted_spans"], 1);
    assert_eq!(body["steps"][2]["route"], "token_factory");
    assert_eq!(
        harness
            .token_factory_calls
            .lock()
            .expect("token factory calls")
            .len(),
        2
    );
    assert_eq!(*harness.hermes.mcp_calls.lock().expect("MCP calls"), 1);
    let received = harness.hermes.received.lock().expect("Hermes received");
    let request_text = serde_json::to_string(&received[0]).expect("request text");
    assert!(request_text.contains("Avery Morgan"));
    assert!(request_text.contains("Pru remembers"));
    assert!(pru_ssn::recognize(&request_text).is_empty());
    let turn_id = body["turn_id"].as_str().expect("turn id");
    assert_eq!(
        harness
            .hermes
            .deleted
            .lock()
            .expect("deleted sessions")
            .as_slice(),
        [turn_id]
    );
}

#[tokio::test]
async fn retries_a_temporary_session_cleanup_conflict() {
    let harness = harness().await;
    *harness
        .hermes
        .delete_conflicts_remaining
        .lock()
        .expect("delete conflict counter lock") = 1;
    mint_use(&harness).await;

    let (status, body) = chat(&harness, "Summarize the selected TEST DATA client").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["answer"], "The TEST DATA client is Avery Morgan.");
    let turn_id = body["turn_id"].as_str().expect("turn id");
    assert_eq!(
        harness
            .hermes
            .deleted
            .lock()
            .expect("deleted sessions")
            .as_slice(),
        [turn_id]
    );
    assert_eq!(
        *harness
            .hermes
            .delete_conflicts_remaining
            .lock()
            .expect("delete conflict counter lock"),
        0
    );
}

#[tokio::test]
async fn gateway_denial_replaces_the_misleading_hermes_answer_and_deletes_session() {
    let harness = harness().await;

    let (status, body) = chat(&harness, "Summarize the selected TEST DATA client").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["steps"].as_array().expect("steps").len(), 1);
    assert_eq!(body["steps"][0]["decision"], "deny");
    assert_eq!(
        body["steps"][0]["policy_reason"],
        "matching_consent_required"
    );
    assert_eq!(
        body["answer"],
        "Denied by the Pru gateway: matching_consent_required"
    );
    assert!(
        harness
            .token_factory_calls
            .lock()
            .expect("token factory calls")
            .is_empty()
    );
    assert_eq!(*harness.hermes.mcp_calls.lock().expect("MCP calls"), 0);
    let turn_id = body["turn_id"].as_str().expect("turn id");
    assert_eq!(
        harness
            .hermes
            .deleted
            .lock()
            .expect("deleted sessions")
            .as_slice(),
        [turn_id]
    );
}
