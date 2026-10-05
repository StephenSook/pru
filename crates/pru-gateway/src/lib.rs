use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Utc};
use pru_consent::{ConsentVerifier, RevocationList};
use pru_policy::{
    DataResource, EgressAction, EgressAuthorizer, EgressPrincipal, EgressRequest,
    VerifiedConsentFacts,
};
use pru_ssn::SsnSpan;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

pub const TOKEN_FACTORY_MODEL: &str = "nvidia/Nemotron-3_5-Lightning";
pub const TOKEN_FACTORY_RECIPIENT: &str = "Nebius Token Factory";

#[derive(Clone)]
pub struct GatewayState {
    inner: Arc<GatewayInner>,
}

struct GatewayInner {
    consent_verifier: ConsentVerifier,
    revocations: RevocationList,
    authorizer: EgressAuthorizer,
    client: reqwest::Client,
    config: GatewayConfig,
    ledger: Mutex<EgressLedger>,
    clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct GatewayConfig {
    pub local_base_url: String,
    pub local_model: String,
    pub local_api_key: Option<String>,
    pub token_factory_base_url: String,
    pub nebius_api_key: String,
    pub ledger_path: PathBuf,
    pub ledger_hash_key: [u8; 32],
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub client_id: String,
    pub consent_token: String,
    pub purpose: String,
    #[serde(default)]
    pub recipient: Option<String>,
    pub arguments: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionResponse {
    pub decision: String,
    pub route: String,
    pub latency_ms: u128,
    pub dry_run: bool,
    pub contains_ssn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Copy)]
enum GatewayAction {
    SendEmail,
    CreateEvent,
    LlmComplete,
}

impl GatewayAction {
    fn parse(value: &str) -> Result<Self, GatewayError> {
        match value {
            "send_email" => Ok(Self::SendEmail),
            "create_event" => Ok(Self::CreateEvent),
            "llm_complete" => Ok(Self::LlmComplete),
            _ => Err(GatewayError::UnknownAction(value.to_owned())),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::SendEmail => "send_email",
            Self::CreateEvent => "create_event",
            Self::LlmComplete => "llm_complete",
        }
    }
}

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("unknown action: {0}")]
    UnknownAction(String),
    #[error("consent verification failed: {0}")]
    Consent(String),
    #[error("policy denied this action: {0}")]
    PolicyDenied(String),
    #[error("policy engine failed: {0}")]
    Policy(String),
    #[error("upstream request failed: {0}")]
    Upstream(String),
    #[error("egress ledger failed: {0}")]
    Ledger(String),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::UnknownAction(_) => StatusCode::NOT_FOUND,
            Self::Consent(_) | Self::PolicyDenied(_) => StatusCode::FORBIDDEN,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Ledger(_) | Self::Policy(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

#[derive(Debug, Serialize)]
struct LedgerEntry<'a> {
    id: Uuid,
    at: DateTime<Utc>,
    client_id: &'a str,
    action: &'a str,
    decision: &'a str,
    route: &'a str,
    contains_ssn: bool,
    span_hashes: Vec<String>,
}

struct EgressLedger {
    path: PathBuf,
    hash_key: [u8; 32],
}

impl EgressLedger {
    fn new(path: impl AsRef<Path>, hash_key: [u8; 32]) -> Result<Self, GatewayError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                GatewayError::Ledger(format!("create {}: {error}", parent.display()))
            })?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| GatewayError::Ledger(format!("open {}: {error}", path.display())))?;
        Ok(Self { path, hash_key })
    }

    fn append(
        &self,
        at: DateTime<Utc>,
        request: &ActionRequest,
        action: GatewayAction,
        decision: &str,
        route: &str,
        spans: &[DetectedSpan],
    ) -> Result<(), GatewayError> {
        let span_hashes = spans
            .iter()
            .map(|span| {
                blake3::keyed_hash(&self.hash_key, span.matched.as_bytes())
                    .to_hex()
                    .to_string()
            })
            .collect();
        let entry = LedgerEntry {
            id: Uuid::new_v4(),
            at,
            client_id: &request.client_id,
            action: action.name(),
            decision,
            route,
            contains_ssn: !spans.is_empty(),
            span_hashes,
        };
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|error| GatewayError::Ledger(format!("open ledger: {error}")))?;
        serde_json::to_writer(&mut file, &entry)
            .map_err(|error| GatewayError::Ledger(format!("serialize entry: {error}")))?;
        file.write_all(b"\n")
            .map_err(|error| GatewayError::Ledger(format!("append entry: {error}")))?;
        file.flush()
            .map_err(|error| GatewayError::Ledger(format!("flush entry: {error}")))
    }
}

#[derive(Debug)]
struct DetectedSpan {
    matched: String,
    #[allow(dead_code)]
    span: SsnSpan,
}

impl GatewayState {
    pub fn new(
        consent_verifier: ConsentVerifier,
        revocations: RevocationList,
        config: GatewayConfig,
    ) -> Result<Self, GatewayError> {
        Self::with_clock(consent_verifier, revocations, config, Arc::new(SystemClock))
    }

    pub fn with_clock(
        consent_verifier: ConsentVerifier,
        revocations: RevocationList,
        config: GatewayConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, GatewayError> {
        let ledger = EgressLedger::new(&config.ledger_path, config.ledger_hash_key)?;
        let authorizer =
            EgressAuthorizer::new().map_err(|error| GatewayError::Policy(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(GatewayInner {
                consent_verifier,
                revocations,
                authorizer,
                client: reqwest::Client::new(),
                config,
                ledger: Mutex::new(ledger),
                clock,
            }),
        })
    }
}

pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route("/v1/actions/{action}", post(handle_action))
        .with_state(state)
}

async fn handle_action(
    State(state): State<GatewayState>,
    AxumPath(action): AxumPath<String>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, GatewayError> {
    let started = Instant::now();
    let action = GatewayAction::parse(&action)?;
    let spans = scan_value(&request.arguments);
    let contains_ssn = !spans.is_empty();
    let (policy_action, route, recipient, destination_region) = match action {
        GatewayAction::LlmComplete if contains_ssn => (
            EgressAction::Use,
            "local",
            "local llama-server".to_owned(),
            "local".to_owned(),
        ),
        GatewayAction::LlmComplete => (
            EgressAction::Disclose,
            "token_factory",
            TOKEN_FACTORY_RECIPIENT.to_owned(),
            "dynamic".to_owned(),
        ),
        GatewayAction::SendEmail | GatewayAction::CreateEvent => (
            EgressAction::Disclose,
            "dry_run",
            request
                .recipient
                .clone()
                .ok_or_else(|| GatewayError::Consent("recipient is required".to_owned()))?,
            "external".to_owned(),
        ),
    };

    let now = state.inner.clock.now();
    let verified = match state.inner.consent_verifier.verify(
        &request.consent_token,
        now,
        &state.inner.revocations,
    ) {
        Ok(verified) => verified,
        Err(error) => {
            append_ledger(&state, now, &request, action, "deny", route, &spans)?;
            return Err(GatewayError::Consent(error.to_string()));
        }
    };
    if verified.client_id != request.client_id {
        append_ledger(&state, now, &request, action, "deny", route, &spans)?;
        return Err(GatewayError::Consent(
            "token client does not match request client".to_owned(),
        ));
    }
    if verified.purpose != request.purpose {
        append_ledger(&state, now, &request, action, "deny", route, &spans)?;
        return Err(GatewayError::Consent(
            "token purpose does not match request purpose".to_owned(),
        ));
    }

    let facts = VerifiedConsentFacts {
        client_id: verified.client_id.clone(),
        kind: match verified.kind {
            pru_consent::ConsentKind::Use => "use",
            pru_consent::ConsentKind::Disclose => "disclose",
        }
        .to_owned(),
        recipient: verified.recipient.clone().unwrap_or_default(),
        purpose: verified.purpose.clone(),
        expires_at: verified.expires_at,
    };
    let policy_request = EgressRequest {
        principal: EgressPrincipal::PruAgent,
        action: policy_action,
        resource: DataResource {
            id: Uuid::new_v4().to_string(),
            client_id: request.client_id.clone(),
            contains_ssn,
            destination_region,
            recipient: recipient.clone(),
        },
        purpose: request.purpose.clone(),
        consent_facts: vec![facts],
        now,
    };
    let policy_decision = state
        .inner
        .authorizer
        .authorize(&policy_request)
        .map_err(|error| GatewayError::PolicyDenied(error.to_string()))?;
    if !policy_decision.allowed {
        let diagnostics = policy_decision.diagnostics.join("; ");
        append_ledger(&state, now, &request, action, "deny", route, &spans)?;
        return Err(GatewayError::PolicyDenied(diagnostics));
    }

    // Record the authorization decision before any permitted side effect.
    append_ledger(&state, now, &request, action, "permit", route, &spans)?;

    let (dry_run, result) = match action {
        GatewayAction::SendEmail | GatewayAction::CreateEvent => (
            true,
            Some(json!({
                "method": "POST",
                "recipient": recipient,
                "body": request.arguments,
                "sent": false
            })),
        ),
        GatewayAction::LlmComplete => (
            false,
            Some(call_llm(&state, route, request.arguments).await?),
        ),
    };
    Ok(Json(ActionResponse {
        decision: "permit".to_owned(),
        route: route.to_owned(),
        latency_ms: started.elapsed().as_millis(),
        dry_run,
        contains_ssn,
        result,
    }))
}

fn append_ledger(
    state: &GatewayState,
    now: DateTime<Utc>,
    request: &ActionRequest,
    action: GatewayAction,
    decision: &str,
    route: &str,
    spans: &[DetectedSpan],
) -> Result<(), GatewayError> {
    state
        .inner
        .ledger
        .lock()
        .map_err(|_| GatewayError::Ledger("ledger lock poisoned".to_owned()))?
        .append(now, request, action, decision, route, spans)
}

async fn call_llm(
    state: &GatewayState,
    route: &str,
    mut body: Value,
) -> Result<Value, GatewayError> {
    let (base_url, model, key) = if route == "local" {
        (
            &state.inner.config.local_base_url,
            state.inner.config.local_model.as_str(),
            state.inner.config.local_api_key.as_deref(),
        )
    } else {
        (
            &state.inner.config.token_factory_base_url,
            TOKEN_FACTORY_MODEL,
            Some(state.inner.config.nebius_api_key.as_str()),
        )
    };
    body["model"] = Value::String(model.to_owned());
    if route != "local" {
        body["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }
    let mut request = state.inner.client.post(format!(
        "{}/chat/completions",
        base_url.trim_end_matches('/')
    ));
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|error| GatewayError::Upstream(error.to_string()))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| GatewayError::Upstream(format!("decode {status}: {error}")))?;
    if !status.is_success() {
        return Err(GatewayError::Upstream(format!(
            "upstream returned {status}: {value}"
        )));
    }
    Ok(value)
}

fn scan_value(value: &Value) -> Vec<DetectedSpan> {
    let mut detected = Vec::new();
    scan_value_into(value, None, &mut detected);
    detected
}

fn scan_value_into(value: &Value, field_name: Option<&str>, detected: &mut Vec<DetectedSpan>) {
    match value {
        Value::String(text) => {
            let direct = pru_ssn::recognize(text);
            if direct.is_empty() {
                if let Some(field_name) = field_name {
                    let contextual = format!("{field_name}: {text}");
                    record_spans(&contextual, pru_ssn::recognize(&contextual), detected);
                }
            } else {
                record_spans(text, direct, detected);
            }
        }
        Value::Array(values) => {
            for value in values {
                scan_value_into(value, field_name, detected);
            }
        }
        Value::Object(values) => {
            for (name, value) in values {
                scan_value_into(value, Some(name), detected);
            }
        }
        Value::Number(number) => {
            if let Some(field_name) = field_name {
                let contextual = format!("{field_name}: {number}");
                record_spans(&contextual, pru_ssn::recognize(&contextual), detected);
            }
        }
        Value::Null | Value::Bool(_) => {}
    }
}

fn record_spans(text: &str, spans: Vec<SsnSpan>, detected: &mut Vec<DetectedSpan>) {
    for span in spans {
        detected.push(DetectedSpan {
            matched: text[span.start..span.end].to_owned(),
            span,
        });
    }
}

pub fn read_ledger(path: impl AsRef<Path>) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub fn unique_span_hashes(path: impl AsRef<Path>) -> std::io::Result<BTreeSet<String>> {
    let contents = read_ledger(path)?;
    let mut hashes = BTreeSet::new();
    for line in contents.lines() {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(values) = value["span_hashes"].as_array() {
            hashes.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    Ok(hashes)
}

pub fn ensure_file_exists(path: impl AsRef<Path>) -> std::io::Result<()> {
    File::create(path).map(drop)
}
