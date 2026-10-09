use chrono::Utc;
use pru_consent::ConsentKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    ActionRequest, GatewayError,
    api::{
        ApiError, ApiState, DemoClient, StoredConsent, read_consents, read_demo_client,
        reject_potentially_issued_ssns,
    },
    workspace::WorkspaceContext,
};

const MAX_INPUT_CHARS: usize = 4_000;
const MAX_MODEL_CALLS: usize = 4;
const MAX_TOOL_CALLS: usize = 4;
const MAX_OUTPUT_TOKENS: u64 = 2_000;
const TOKENS_PER_CALL: u64 = 500;
const DEFAULT_PURPOSE: &str = "prepare this synthetic tax client's return";

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub turn_id: Uuid,
    pub answer: String,
    pub steps: Vec<ChatStep>,
}

#[derive(Debug, Serialize)]
pub struct ChatStep {
    pub kind: String,
    pub route: String,
    pub model_id: Option<String>,
    pub latency_ms: u128,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub decision: String,
    pub policy_reason: String,
    pub ledger_entry_id: Uuid,
    pub dry_run: bool,
    pub tool_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DraftEmail {
    to: String,
    subject: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct ProposedCalendar {
    title: String,
    start: String,
    end: String,
}

pub async fn run_chat(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client_id: &str,
    request: ChatRequest,
) -> Result<ChatResponse, ApiError> {
    reject_potentially_issued_ssns([request.message.as_str()])?;
    state.ensure_demo_client(client_id)?;
    if request.message.chars().count() > MAX_INPUT_CHARS {
        return Err(ApiError::InvalidRequest(format!(
            "message exceeds {MAX_INPUT_CHARS} characters"
        )));
    }
    if request.message.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "message must not be empty".to_owned(),
        ));
    }

    let client = read_demo_client(state, workspace, client_id)?;
    let consents = read_consents(state, workspace, client_id)?;
    let mut messages = vec![
        json!({
            "role": "system",
            "content": "You are Pru in a TEST DATA demo. Use tools for email or calendar requests. Never repeat a Social Security number in the answer. Email and calendar tools are dry-run only."
        }),
        json!({
            "role": "user",
            "content": format!("{}\n\nClient excerpt:\n{}", request.message, select_excerpt(&client, &request.message))
        }),
    ];
    let mut steps = Vec::new();
    let mut total_output_tokens = 0_u64;
    let mut tool_calls = 0_usize;

    for _ in 0..MAX_MODEL_CALLS {
        let remaining = MAX_OUTPUT_TOKENS.saturating_sub(total_output_tokens);
        if remaining == 0 {
            break;
        }
        let max_tokens = remaining.min(TOKENS_PER_CALL);
        let use_consent = select_consent(&consents, ConsentKind::Use, None);
        let purpose = use_consent
            .map(|consent| consent.record.purpose.clone())
            .unwrap_or_else(|| DEFAULT_PURPOSE.to_owned());
        let body = json!({
            "messages": messages.clone(),
            "tools": tool_definitions(),
            "tool_choice": "auto",
            "temperature": 0,
            "max_tokens": max_tokens
        });
        let action_request = ActionRequest {
            client_id: client_id.to_owned(),
            workspace_id: Some(workspace.id()),
            consent_token: use_consent
                .map(|consent| consent.token.clone())
                .unwrap_or_default(),
            purpose,
            recipient: None,
            arguments: body,
        };
        let action = match state
            .gateway()
            .execute_chat_completion(action_request)
            .await
        {
            Ok(action) => action,
            Err(error @ (GatewayError::Consent(_) | GatewayError::PolicyDenied(_))) => {
                let step = denied_step(state, workspace, client_id, "model", None, &error)?;
                let answer = step.policy_reason.clone();
                steps.push(step);
                return Ok(ChatResponse {
                    turn_id: Uuid::new_v4(),
                    answer,
                    steps,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let result = action.result.as_ref().ok_or(ApiError::ModelProtocol)?;
        let usage = result.get("usage").ok_or(ApiError::ModelProtocol)?;
        let input_tokens = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .ok_or(ApiError::ModelProtocol)?;
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .ok_or(ApiError::ModelProtocol)?;
        total_output_tokens = total_output_tokens.saturating_add(output_tokens);
        if total_output_tokens > MAX_OUTPUT_TOKENS {
            return Err(ApiError::ModelProtocol);
        }
        let model_id = action.model_id.clone();
        steps.push(ChatStep {
            kind: "model".to_owned(),
            route: action.route.clone(),
            model_id: model_id.clone(),
            latency_ms: action.latency_ms,
            input_tokens,
            output_tokens,
            cost_usd: model_cost(&action.route, input_tokens, output_tokens),
            decision: action.decision.clone(),
            policy_reason: action.policy_reason.clone(),
            ledger_entry_id: action.ledger_entry_id,
            dry_run: false,
            tool_name: None,
        });

        let message = result
            .pointer("/choices/0/message")
            .and_then(Value::as_object)
            .ok_or(ApiError::ModelProtocol)?;
        let calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if calls.is_empty() {
            let answer = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or(ApiError::ModelProtocol)?;
            return Ok(ChatResponse {
                turn_id: Uuid::new_v4(),
                answer: redact_ssns(answer),
                steps,
            });
        }

        tool_calls = tool_calls.saturating_add(calls.len());
        if tool_calls > MAX_TOOL_CALLS {
            return Err(ApiError::ModelProtocol);
        }
        messages.push(Value::Object(message.clone()));
        for call in calls {
            let tool_result = execute_tool(state, workspace, client_id, &consents, &call).await?;
            let tool_call_id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or(ApiError::ModelProtocol)?;
            let tool_name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or(ApiError::ModelProtocol)?;
            let denied = tool_result.decision == "deny";
            steps.push(tool_result);
            if denied {
                let answer = steps
                    .last()
                    .map(|step| step.policy_reason.clone())
                    .unwrap_or_else(|| "policy denied the dry run".to_owned());
                return Ok(ChatResponse {
                    turn_id: Uuid::new_v4(),
                    answer,
                    steps,
                });
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "name": tool_name,
                "content": "Dry run permitted. Nothing was sent."
            }));
        }
    }

    Ok(ChatResponse {
        turn_id: Uuid::new_v4(),
        answer: "The bounded turn stopped after four model calls.".to_owned(),
        steps,
    })
}

fn select_excerpt(client: &DemoClient, message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("ssn") || lower.contains("social security") {
        format!(
            "TEST DATA\nName: {}\nFiling status: {}\nSSN: {}",
            client.name, client.filing_status, client.ssn
        )
    } else if lower.contains("w-2") || lower.contains("w2") {
        format!(
            "TEST DATA\nName: {}\nEmail: {}\n{}",
            client.name, client.email, client.w2_line
        )
    } else {
        format!("TEST DATA\nName: {}\nEmail: {}", client.name, client.email)
    }
}

fn select_consent<'a>(
    consents: &'a [StoredConsent],
    kind: ConsentKind,
    recipient: Option<&str>,
) -> Option<&'a StoredConsent> {
    let now = Utc::now();
    consents.iter().find(|consent| {
        !consent.revoked
            && consent.record.kind == kind
            && consent.record.expires_at > now
            && match recipient {
                Some(recipient) => consent.record.recipient.as_deref() == Some(recipient),
                None => true,
            }
    })
}

async fn execute_tool(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client_id: &str,
    consents: &[StoredConsent],
    call: &Value,
) -> Result<ChatStep, ApiError> {
    let name = call
        .pointer("/function/name")
        .and_then(Value::as_str)
        .ok_or(ApiError::ModelProtocol)?;
    let arguments = call
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .ok_or(ApiError::ModelProtocol)?;
    let (action_name, recipient, action_arguments) = match name {
        "draft_email" => {
            let draft: DraftEmail =
                serde_json::from_str(arguments).map_err(|_| ApiError::ModelProtocol)?;
            (
                "send_email",
                draft.to.clone(),
                json!({"to": draft.to, "subject": draft.subject, "body": draft.body}),
            )
        }
        "propose_calendar" => {
            let proposal: ProposedCalendar =
                serde_json::from_str(arguments).map_err(|_| ApiError::ModelProtocol)?;
            (
                "create_event",
                "practice calendar".to_owned(),
                json!({"title": proposal.title, "start": proposal.start, "end": proposal.end}),
            )
        }
        _ => return Err(ApiError::ModelProtocol),
    };
    let consent = select_consent(consents, ConsentKind::Disclose, Some(&recipient))
        .or_else(|| select_consent(consents, ConsentKind::Use, None));
    let purpose = consent
        .map(|consent| consent.record.purpose.clone())
        .unwrap_or_else(|| DEFAULT_PURPOSE.to_owned());
    let request = ActionRequest {
        client_id: client_id.to_owned(),
        workspace_id: Some(workspace.id()),
        consent_token: consent
            .map(|consent| consent.token.clone())
            .unwrap_or_default(),
        purpose,
        recipient: Some(recipient),
        arguments: action_arguments,
    };
    match state.gateway().execute_action(action_name, request).await {
        Ok(action) => Ok(ChatStep {
            kind: "tool".to_owned(),
            route: action.route,
            model_id: None,
            latency_ms: action.latency_ms,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            decision: action.decision,
            policy_reason: action.policy_reason,
            ledger_entry_id: action.ledger_entry_id,
            dry_run: action.dry_run,
            tool_name: Some(name.to_owned()),
        }),
        Err(error @ (GatewayError::Consent(_) | GatewayError::PolicyDenied(_))) => {
            denied_step(state, workspace, client_id, "tool", Some(name), &error)
        }
        Err(error) => Err(error.into()),
    }
}

fn denied_step(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client_id: &str,
    kind: &str,
    tool_name: Option<&str>,
    error: &GatewayError,
) -> Result<ChatStep, ApiError> {
    let entry = state
        .gateway()
        .ledger_entries_in_workspace(&workspace.id(), client_id)?
        .into_iter()
        .next()
        .ok_or(ApiError::ModelProtocol)?;
    let policy_reason = entry["policy_reason"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    Ok(ChatStep {
        kind: kind.to_owned(),
        route: entry["route"].as_str().unwrap_or("unknown").to_owned(),
        model_id: entry["model_id"].as_str().map(str::to_owned),
        latency_ms: 0,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        decision: "deny".to_owned(),
        policy_reason,
        ledger_entry_id: serde_json::from_value(entry["id"].clone())
            .map_err(|_| ApiError::ModelProtocol)?,
        dry_run: false,
        tool_name: tool_name.map(str::to_owned),
    })
}

fn model_cost(route: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    if route == "local" {
        0.0
    } else {
        (input_tokens as f64 * 0.06 + output_tokens as f64 * 0.24) / 1_000_000.0
    }
}

fn redact_ssns(text: &str) -> String {
    let mut output = text.to_owned();
    let mut spans = pru_ssn::recognize(text);
    spans.sort_by_key(|span| span.start);
    for span in spans.into_iter().rev() {
        output.replace_range(span.start..span.end, "[SSN kept local]");
    }
    output
}

fn tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "draft_email",
                "description": "Create an email draft. This never sends.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["to", "subject", "body"],
                    "properties": {
                        "to": {"type": "string"},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "propose_calendar",
                "description": "Create a calendar proposal. This never sends.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "start", "end"],
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string"},
                        "end": {"type": "string"}
                    }
                }
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_route_has_zero_cost_and_remote_route_uses_list_prices() {
        assert_eq!(model_cost("local", 1_000, 1_000), 0.0);
        assert!((model_cost("token_factory", 1_000_000, 1_000_000) - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn redaction_removes_detected_ssn_text() {
        let redacted = redact_ssns("The synthetic SSN is 101-00-1001.");
        assert_eq!(redacted, "The synthetic SSN is [SSN kept local].");
    }

    #[test]
    fn selected_excerpts_only_include_ssn_for_an_explicit_ssn_request() {
        let client = DemoClient {
            id: "demo".to_owned(),
            name: "Demo Person".to_owned(),
            filing_status: "Single".to_owned(),
            w2_line: "W-2 wages: $1".to_owned(),
            ssn: "101-00-1001".to_owned(),
            email: "demo@example.com".to_owned(),
            test_data: true,
        };
        assert!(!select_excerpt(&client, "Draft a 1099 email").contains(&client.ssn));
        assert!(!select_excerpt(&client, "Email W-2 details").contains(&client.ssn));
        assert!(select_excerpt(&client, "What is the SSN?").contains(&client.ssn));
    }

    #[test]
    fn token_factory_model_constant_matches_the_costed_model() {
        assert_eq!(crate::TOKEN_FACTORY_MODEL, "nvidia/Nemotron-3_5-Lightning");
    }
}
