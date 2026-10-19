use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    ActionRequest, GatewayError, TOKEN_FACTORY_MODEL,
    agent::{AgentTurnError, LedgerContext},
    api::ApiState,
};

const MAX_AGENT_REQUEST_BODY_BYTES: usize = 2_000_000;
const MAX_AGENT_OUTPUT_TOKENS: u64 = 600;
const AGENT_TURN_HEADER: &str = "x-pru-turn";

pub fn agent_router(state: ApiState) -> Router {
    Router::new()
        .route("/agent/v1/models", get(models))
        .route("/agent/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

async fn models(State(state): State<ApiState>, request: Request) -> Response {
    if !authenticated(&state, &request) {
        return invalid_credential();
    }
    Json(json!({
        "object": "list",
        "data": [{
            "id": TOKEN_FACTORY_MODEL,
            "object": "model",
            "owned_by": "pru",
            "context_length": 131_072
        }]
    }))
    .into_response()
}

async fn chat_completions(State(state): State<ApiState>, request: Request) -> Response {
    if !authenticated(&state, &request) {
        return invalid_credential();
    }
    let turn_id = match turn_id(&request) {
        Some(turn_id) => turn_id,
        None => {
            return openai_error(
                StatusCode::CONFLICT,
                "turn_mismatch",
                "The request does not identify the active Hermes turn.",
            );
        }
    };
    let turn = match state.agent_turns().reserve_model_call(turn_id) {
        Ok(turn) => turn,
        Err(error) => return agent_turn_error(error),
    };
    let body = match to_bytes(request.into_body(), MAX_AGENT_REQUEST_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return openai_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "The agent model request is too large.",
            );
        }
    };
    let body = match serde_json::from_slice::<Value>(&body) {
        Ok(body) => body,
        Err(error) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Invalid JSON request: {error}"),
            );
        }
    };
    let arguments = match normalize_body(&body) {
        Ok(arguments) => arguments,
        Err(message) => {
            return openai_error(StatusCode::BAD_REQUEST, "invalid_request", message);
        }
    };
    let action = ActionRequest {
        client_id: turn.client_id,
        workspace_id: Some(turn.workspace.id().to_owned()),
        consent_token: turn.consent_token,
        purpose: turn.purpose,
        recipient: None,
        arguments,
        ledger_context: Some(LedgerContext {
            turn_id,
            actor: "hermes".to_owned(),
            step: "model".to_owned(),
            tool: None,
            redacted_source: None,
        }),
    };
    match state.gateway().execute_agent_model(action).await {
        Ok(response) => {
            let result = response.result.unwrap_or(Value::Null);
            if let Err(error) = state
                .agent_turns()
                .replace_pending_tool_calls(turn_id, &result)
            {
                return agent_turn_error(error);
            }
            Json(result).into_response()
        }
        Err(error) => gateway_error(error),
    }
}

fn authenticated(state: &ApiState, request: &Request) -> bool {
    let bearer = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    state.agent_credential().matches_bearer(bearer)
}

fn invalid_credential() -> Response {
    openai_error(
        StatusCode::UNAUTHORIZED,
        "invalid_api_key",
        "The agent credential is invalid.",
    )
}

fn turn_id(request: &Request) -> Option<Uuid> {
    request
        .headers()
        .get(AGENT_TURN_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn normalize_body(body: &Value) -> Result<Value, &'static str> {
    let object = body
        .as_object()
        .ok_or("The request body must be a JSON object.")?;
    let messages = object
        .get("messages")
        .filter(|value| value.is_array())
        .ok_or("The request must contain a messages array.")?;
    let mut normalized = Map::new();
    normalized.insert("messages".to_owned(), messages.clone());
    for key in ["tools", "tool_choice"] {
        if let Some(value) = object.get(key) {
            normalized.insert(key.to_owned(), value.clone());
        }
    }
    let max_tokens = object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_AGENT_OUTPUT_TOKENS)
        .min(MAX_AGENT_OUTPUT_TOKENS);
    normalized.insert("max_tokens".to_owned(), Value::from(max_tokens));
    normalized.insert("temperature".to_owned(), Value::from(0));
    normalized.insert(
        "chat_template_kwargs".to_owned(),
        json!({"enable_thinking": false}),
    );
    normalized.insert(
        "model".to_owned(),
        Value::String(TOKEN_FACTORY_MODEL.to_owned()),
    );
    Ok(Value::Object(normalized))
}

fn gateway_error(error: GatewayError) -> Response {
    let message = error.to_string();
    let status = error.into_response().status();
    let code = match status {
        StatusCode::FORBIDDEN => "policy_denied",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_exceeded",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        StatusCode::BAD_GATEWAY => "upstream_error",
        _ => "gateway_error",
    };
    openai_error(status, code, &message)
}

fn agent_turn_error(error: AgentTurnError) -> Response {
    let message = error.to_string();
    let status = error.into_response().status();
    let code = if status == StatusCode::TOO_MANY_REQUESTS {
        "rate_limit_exceeded"
    } else if status == StatusCode::CONFLICT {
        "turn_mismatch"
    } else {
        "gateway_error"
    };
    openai_error(status, code, &message)
}

fn openai_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "pru_gateway_error",
                "code": code
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_keeps_only_the_supported_request_fields() {
        let normalized = normalize_body(&json!({
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function"}],
            "tool_choice": "auto",
            "max_tokens": 900,
            "temperature": 0.8,
            "reasoning_effort": "medium",
            "stream": true,
            "user": "discard-me"
        }))
        .expect("normalized body");
        assert_eq!(normalized["max_tokens"], 600);
        assert_eq!(normalized["temperature"], 0);
        assert_eq!(normalized["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(normalized["model"], TOKEN_FACTORY_MODEL);
        assert!(normalized.get("tools").is_some());
        assert!(normalized.get("tool_choice").is_some());
        assert!(normalized.get("reasoning_effort").is_none());
        assert!(normalized.get("stream").is_none());
        assert!(normalized.get("user").is_none());
    }
}
