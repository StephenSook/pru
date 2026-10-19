use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use pru_consent::ConsentKind;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use serde_json::{Map, Value, json};

use crate::{
    ActionRequest, GatewayError,
    agent::{ActiveTurnSnapshot, LedgerContext},
    api::{ApiState, StoredConsent, read_consents, read_demo_client},
};

pub const MCP_TOOL_NAMES: [&str; 7] = [
    "get_client_summary",
    "get_consent_state",
    "read_document",
    "ask_local_model",
    "draft_client_email",
    "propose_calendar_slot",
    "remember_note",
];

#[derive(Clone)]
struct PruMcpServer {
    state: ApiState,
}

pub fn mcp_router(state: ApiState) -> axum::Router {
    let factory_state = state.clone();
    let service: StreamableHttpService<PruMcpServer, NeverSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(PruMcpServer {
                    state: factory_state.clone(),
                })
            },
            Arc::new(NeverSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .with_allowed_hosts(["127.0.0.1", "localhost", "[::1]"]),
        );
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_guard))
}

async fn mcp_auth_guard(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if state.agent_credential().matches_bearer(bearer) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "The agent credential is invalid."})),
        )
            .into_response()
    }
}

impl ServerHandler for PruMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Pru's seven consent-gated tax-practice tools")
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(tool_definitions())))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tool_definitions()
            .into_iter()
            .find(|tool| tool.name == name)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
        let server = self.clone();
        async move {
            let name = request.name.into_owned();
            let arguments = Value::Object(request.arguments.unwrap_or_default());
            let turn = match server
                .state
                .agent_turns()
                .consume_pending_tool_call(&name, arguments.clone())
            {
                Ok(turn) => turn,
                Err(error) => return Ok(tool_error(error.to_string()).into()),
            };
            let result = match server.execute_tool(&turn, &name, arguments).await {
                Ok(result) => match ensure_no_ssn(&result) {
                    Ok(()) => {
                        let mut success = CallToolResult::success(vec![ContentBlock::text(
                            serde_json::to_string(&result)
                                .unwrap_or_else(|_| "Tool result encoding failed".to_owned()),
                        )]);
                        success.structured_content = Some(result);
                        success
                    }
                    Err(message) => {
                        let _ = server.state.agent_turns().terminate(turn.turn_id);
                        tool_error(message)
                    }
                },
                Err(message) => {
                    let _ = server.state.agent_turns().terminate(turn.turn_id);
                    tool_error(message)
                }
            };
            Ok(result.into())
        }
    }
}

impl PruMcpServer {
    async fn execute_tool(
        &self,
        turn: &ActiveTurnSnapshot,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        match name {
            "get_client_summary" => self.get_client_summary(turn).await,
            "get_consent_state" => self.get_consent_state(turn).await,
            "read_document" => self.read_document(turn, &arguments).await,
            "ask_local_model" => self.ask_local_model(turn, &arguments).await,
            "draft_client_email" => self.draft_client_email(turn, &arguments).await,
            "propose_calendar_slot" => self.propose_calendar_slot(turn, &arguments).await,
            "remember_note" => self.remember_note(turn, &arguments).await,
            _ => Err(format!("unknown Pru tool: {name}")),
        }
    }

    async fn get_client_summary(&self, turn: &ActiveTurnSnapshot) -> Result<Value, String> {
        let client = read_demo_client(&self.state, &turn.workspace, &turn.client_id)
            .map_err(|error| error.to_string())?;
        let raw = serde_json::to_value(&client).map_err(|error| error.to_string())?;
        let result = json!({
            "test_data": client.test_data,
            "name": client.name,
            "email": client.email,
            "filing_status": client.filing_status,
            "w2": client.w2_line,
            "ssn": "[SSN kept local]"
        });
        self.agent_use(turn, "get_client_summary", result, Some(raw))
            .await
            .map(|response| response.0)
    }

    async fn get_consent_state(&self, turn: &ActiveTurnSnapshot) -> Result<Value, String> {
        let consents = read_consents(&self.state, &turn.workspace, &turn.client_id)
            .map_err(|error| error.to_string())?;
        let result = Value::Array(
            consents
                .iter()
                .map(|consent| {
                    json!({
                        "kind": consent.record.kind,
                        "recipient": consent.record.recipient,
                        "purpose": consent.record.purpose,
                        "expires_at": consent.record.expires_at,
                        "revoked": consent.revoked
                    })
                })
                .collect(),
        );
        self.agent_use(turn, "get_consent_state", result, None)
            .await
            .map(|response| response.0)
    }

    async fn read_document(
        &self,
        turn: &ActiveTurnSnapshot,
        arguments: &Value,
    ) -> Result<Value, String> {
        require_string(arguments, "name")
            .filter(|name| *name == "w2")
            .ok_or_else(|| "read_document supports only name=w2".to_owned())?;
        let raw = self
            .state
            .read_w2_document(&turn.workspace, &turn.client_id)
            .map_err(|error| error.to_string())?;
        let result = redact_value(raw.clone());
        self.agent_use(turn, "read_document", result, Some(raw))
            .await
            .map(|response| response.0)
    }

    async fn ask_local_model(
        &self,
        turn: &ActiveTurnSnapshot,
        arguments: &Value,
    ) -> Result<Value, String> {
        let question = require_string(arguments, "question")
            .ok_or_else(|| "ask_local_model requires question".to_owned())?;
        let client = read_demo_client(&self.state, &turn.workspace, &turn.client_id)
            .map_err(|error| error.to_string())?;
        let action = self
            .state
            .gateway()
            .execute_action(
                "llm_complete",
                ActionRequest {
                    client_id: turn.client_id.clone(),
                    workspace_id: Some(turn.workspace.id()),
                    consent_token: turn.consent_token.clone(),
                    purpose: turn.purpose.clone(),
                    recipient: None,
                    arguments: json!({
                        "messages": [
                            {"role": "system", "content": "Answer only from this TEST DATA client record."},
                            {"role": "user", "content": format!("{question}\n\nTEST DATA client: {}", serde_json::to_string(&client).map_err(|error| error.to_string())?)}
                        ],
                        "temperature": 0,
                        "max_tokens": 600
                    }),
                    ledger_context: Some(tool_context(turn, "ask_local_model", None)),
                },
            )
            .await
            .map_err(|error| self.gateway_reason(turn, error))?;
        let raw_answer = action
            .result
            .as_ref()
            .and_then(|result| result.pointer("/choices/0/message/content"))
            .and_then(Value::as_str)
            .ok_or_else(|| "local model response was invalid".to_owned())?
            .to_owned();
        let result = json!({"answer": redact_text(&raw_answer)});
        self.agent_use(
            turn,
            "ask_local_model",
            result,
            Some(json!({"answer": raw_answer})),
        )
        .await
        .map(|response| response.0)
    }

    async fn draft_client_email(
        &self,
        turn: &ActiveTurnSnapshot,
        arguments: &Value,
    ) -> Result<Value, String> {
        let to = require_string(arguments, "to")
            .ok_or_else(|| "draft_client_email requires to".to_owned())?;
        require_string(arguments, "subject")
            .ok_or_else(|| "draft_client_email requires subject".to_owned())?;
        require_string(arguments, "body")
            .ok_or_else(|| "draft_client_email requires body".to_owned())?;
        let (token, purpose) = self.disclosure_or_use(turn, to)?;
        let action = self
            .state
            .gateway()
            .execute_action(
                "send_email",
                ActionRequest {
                    client_id: turn.client_id.clone(),
                    workspace_id: Some(turn.workspace.id()),
                    consent_token: token,
                    purpose,
                    recipient: Some(to.to_owned()),
                    arguments: arguments.clone(),
                    ledger_context: Some(tool_context(turn, "draft_client_email", None)),
                },
            )
            .await
            .map_err(|error| self.gateway_reason(turn, error))?;
        Ok(redact_value(action.result.unwrap_or(Value::Null)))
    }

    async fn propose_calendar_slot(
        &self,
        turn: &ActiveTurnSnapshot,
        arguments: &Value,
    ) -> Result<Value, String> {
        for field in ["title", "start", "end"] {
            require_string(arguments, field)
                .ok_or_else(|| format!("propose_calendar_slot requires {field}"))?;
        }
        let recipient = "practice calendar";
        let (token, purpose) = self.disclosure_or_use(turn, recipient)?;
        let action = self
            .state
            .gateway()
            .execute_action(
                "create_event",
                ActionRequest {
                    client_id: turn.client_id.clone(),
                    workspace_id: Some(turn.workspace.id()),
                    consent_token: token,
                    purpose,
                    recipient: Some(recipient.to_owned()),
                    arguments: arguments.clone(),
                    ledger_context: Some(tool_context(turn, "propose_calendar_slot", None)),
                },
            )
            .await
            .map_err(|error| self.gateway_reason(turn, error))?;
        Ok(redact_value(action.result.unwrap_or(Value::Null)))
    }

    async fn remember_note(
        &self,
        turn: &ActiveTurnSnapshot,
        arguments: &Value,
    ) -> Result<Value, String> {
        let text = require_string(arguments, "text")
            .ok_or_else(|| "remember_note requires text".to_owned())?
            .to_owned();
        let (_, ledger_entry_id) = self
            .agent_use(turn, "remember_note", arguments.clone(), None)
            .await?;
        let note = self
            .state
            .append_agent_note(&turn.workspace, &turn.client_id, text, ledger_entry_id)
            .map_err(|error| error.to_string())?;
        Ok(json!({"status": "Saved", "note_id": note.id}))
    }

    async fn agent_use(
        &self,
        turn: &ActiveTurnSnapshot,
        tool: &str,
        arguments: Value,
        redacted_source: Option<Value>,
    ) -> Result<(Value, uuid::Uuid), String> {
        let action = self
            .state
            .gateway()
            .execute_agent_tool(ActionRequest {
                client_id: turn.client_id.clone(),
                workspace_id: Some(turn.workspace.id()),
                consent_token: turn.consent_token.clone(),
                purpose: turn.purpose.clone(),
                recipient: None,
                arguments,
                ledger_context: Some(tool_context(turn, tool, redacted_source)),
            })
            .await
            .map_err(|error| self.gateway_reason(turn, error))?;
        Ok((action.result.unwrap_or(Value::Null), action.ledger_entry_id))
    }

    fn disclosure_or_use(
        &self,
        turn: &ActiveTurnSnapshot,
        recipient: &str,
    ) -> Result<(String, String), String> {
        let consents = read_consents(&self.state, &turn.workspace, &turn.client_id)
            .map_err(|error| error.to_string())?;
        Ok(
            active_consent(&consents, ConsentKind::Disclose, Some(recipient)).map_or_else(
                || (turn.consent_token.clone(), turn.purpose.clone()),
                |consent| (consent.token.clone(), consent.record.purpose.clone()),
            ),
        )
    }

    fn gateway_reason(&self, turn: &ActiveTurnSnapshot, error: GatewayError) -> String {
        if matches!(
            error,
            GatewayError::Consent(_) | GatewayError::PolicyDenied(_)
        ) && let Ok(Some(reason)) = self
            .state
            .gateway()
            .ledger_entries_in_workspace(&turn.workspace.id(), &turn.client_id)
            .map(|entries| {
                entries
                    .first()
                    .and_then(|entry| entry["policy_reason"].as_str())
                    .map(str::to_owned)
            })
        {
            return reason;
        }
        error.to_string()
    }
}

fn active_consent<'a>(
    consents: &'a [StoredConsent],
    kind: ConsentKind,
    recipient: Option<&str>,
) -> Option<&'a StoredConsent> {
    let now = Utc::now();
    consents.iter().find(|consent| {
        !consent.revoked
            && consent.record.kind == kind
            && consent.record.expires_at > now
            && recipient.is_none_or(|value| consent.record.recipient.as_deref() == Some(value))
    })
}

fn tool_context(
    turn: &ActiveTurnSnapshot,
    tool: &str,
    redacted_source: Option<Value>,
) -> LedgerContext {
    LedgerContext {
        turn_id: turn.turn_id,
        actor: "hermes".to_owned(),
        step: "tool".to_owned(),
        tool: Some(tool.to_owned()),
        redacted_source,
    }
}

fn require_string<'a>(arguments: &'a Value, field: &str) -> Option<&'a str> {
    arguments.get(field).and_then(Value::as_str)
}

fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

fn ensure_no_ssn(value: &Value) -> Result<(), String> {
    if crate::scan_value(value).is_empty() {
        Ok(())
    } else {
        Err("Pru refused a tool result that still contained an SSN.".to_owned())
    }
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text(&text)),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, redact_value(value)))
                .collect(),
        ),
        scalar => scalar,
    }
}

fn redact_text(text: &str) -> String {
    let mut output = text.to_owned();
    let mut spans = pru_ssn::recognize(text);
    spans.sort_by_key(|span| span.start);
    for span in spans.into_iter().rev() {
        output.replace_range(span.start..span.end, "[SSN kept local]");
    }
    output
}

fn tool_definitions() -> Vec<Tool> {
    vec![
        tool(
            "get_client_summary",
            "Read the selected TEST DATA client's redacted summary.",
            empty_schema(),
            true,
        ),
        tool(
            "get_consent_state",
            "List consent facts for the selected client without returning tokens.",
            empty_schema(),
            true,
        ),
        tool(
            "read_document",
            "Read one redacted TEST DATA client document.",
            object_schema(&["name"], &[("name", "string")]),
            true,
        ),
        tool(
            "ask_local_model",
            "Ask the local model a question that may require the client's synthetic SSN.",
            object_schema(&["question"], &[("question", "string")]),
            true,
        ),
        tool(
            "draft_client_email",
            "Create an email dry run. This never sends.",
            object_schema(
                &["to", "subject", "body"],
                &[("to", "string"), ("subject", "string"), ("body", "string")],
            ),
            false,
        ),
        tool(
            "propose_calendar_slot",
            "Create a calendar dry run. This never sends.",
            object_schema(
                &["title", "start", "end"],
                &[("title", "string"), ("start", "string"), ("end", "string")],
            ),
            false,
        ),
        tool(
            "remember_note",
            "Save one short per-client note through the Pru gateway.",
            object_schema(&["text"], &[("text", "string")]),
            false,
        ),
    ]
}

fn tool(
    name: &'static str,
    description: &'static str,
    schema: Map<String, Value>,
    read_only: bool,
) -> Tool {
    Tool::new(name, description, Arc::new(schema)).with_annotations(
        ToolAnnotations::new()
            .read_only(read_only)
            .destructive(false)
            .idempotent(read_only)
            .open_world(false),
    )
}

fn empty_schema() -> Map<String, Value> {
    object_schema(&[], &[])
}

fn object_schema(required: &[&str], fields: &[(&str, &str)]) -> Map<String, Value> {
    let properties = fields
        .iter()
        .map(|(name, kind)| ((*name).to_owned(), json!({"type": kind})))
        .collect::<Map<_, _>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties
    })
    .as_object()
    .cloned()
    .expect("tool schema is an object")
}
