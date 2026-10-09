use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Extension, Path as AxumPath, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord, RevocationList};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::chat::{ChatRequest, ChatResponse, run_chat};
use crate::workspace::{WORKSPACE_HEADER, WorkspaceContext, WorkspaceError, WorkspaceManager};
use crate::{GatewayError, GatewayState, validate_client_id};

pub const REAL_SSN_REFUSAL_MESSAGE: &str =
    "This public demo accepts test data only. Do not enter real taxpayer data. 26 U.S.C. 7216.";
pub const MAX_REQUEST_BODY_BYTES: usize = 16_384;

#[derive(Clone)]
pub struct ApiState {
    gateway: GatewayState,
    authority: Arc<ConsentAuthority>,
    workspaces: WorkspaceManager,
    storage_lock: Arc<Mutex<()>>,
}

impl ApiState {
    #[must_use]
    pub fn new(gateway: GatewayState, authority: ConsentAuthority, data_root: PathBuf) -> Self {
        Self {
            gateway,
            authority: Arc::new(authority),
            workspaces: WorkspaceManager::new(data_root),
            storage_lock: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn gateway(&self) -> &GatewayState {
        &self.gateway
    }

    pub fn reset_demo(
        &self,
        workspace: &WorkspaceContext,
    ) -> Result<Vec<DemoClientView>, ApiError> {
        let _guard = self
            .storage_lock
            .lock()
            .map_err(|_| ApiError::Storage("demo storage lock poisoned".to_owned()))?;
        for client in demo_clients() {
            for consent in read_consents_unlocked(workspace.root(), &client.id)? {
                self.gateway.revoke(consent.revocation_id)?;
            }
            let directory = client_directory(workspace.root(), &client.id)?;
            fs::create_dir_all(&directory).map_err(|error| {
                ApiError::Storage(format!("create {}: {error}", directory.display()))
            })?;
            write_json(&directory.join("client.json"), &client)?;
            write_json(
                &directory.join("consents.json"),
                &Vec::<StoredConsent>::new(),
            )?;
            fs::write(directory.join("ledger.jsonl"), [])
                .map_err(|error| ApiError::Storage(format!("clear ledger: {error}")))?;
        }
        Ok(demo_clients().iter().map(DemoClientView::from).collect())
    }

    pub fn start_workspace_cleanup(&self) -> tokio::task::JoinHandle<()> {
        let workspaces = self.workspaces.clone();
        tokio::spawn(workspaces.cleanup_loop())
    }

    pub(crate) fn ensure_demo_client(&self, client: &str) -> Result<(), ApiError> {
        validate_client_id(client)?;
        if demo_clients()
            .iter()
            .any(|candidate| candidate.id == client)
        {
            Ok(())
        } else {
            Err(ApiError::NotFound)
        }
    }
}

pub fn api_router(state: ApiState) -> Router {
    let workspace_state = state.clone();
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/demo/clients", get(list_demo_clients))
        .route("/v1/demo/reset", post(reset_demo))
        .route(
            "/v1/clients/{client}/consents",
            post(mint_consent).get(list_consents),
        )
        .route(
            "/v1/clients/{client}/consents/{id}/revoke",
            post(revoke_consent),
        )
        .route("/v1/clients/{client}/ledger", get(get_ledger))
        .route("/v1/clients/{client}/chat", post(chat))
        .layer(middleware::from_fn_with_state(
            workspace_state,
            workspace_guard,
        ))
        .layer(middleware::from_fn(visitor_input_guard))
        .with_state(state)
}

async fn workspace_guard(
    State(state): State<ApiState>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let workspace_id = match workspace_header(request.headers()) {
        Ok(workspace_id) => workspace_id,
        Err(error) => return error.into_response(),
    };
    let exclusive = request.uri().path() == "/v1/demo/reset";
    let (workspace, is_new) = match state.workspaces.acquire(workspace_id, exclusive).await {
        Ok(result) => result,
        Err(error) => return ApiError::Workspace(error).into_response(),
    };
    if is_new && let Err(error) = state.reset_demo(&workspace) {
        state.workspaces.abandon_new(&workspace).await;
        return error.into_response();
    }
    request.extensions_mut().insert(workspace);
    next.run(request).await
}

fn workspace_header(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(WORKSPACE_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Workspace(WorkspaceError::Invalid))
}

pub(crate) async fn visitor_input_guard(request: Request, next: Next) -> Response {
    if request.method() != Method::POST {
        return next.run(request).await;
    }

    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, MAX_REQUEST_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return ApiError::PayloadTooLarge.into_response(),
    };
    if !bytes.is_empty() {
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return ApiError::InvalidRequest("request body must be valid JSON".to_owned())
                    .into_response();
            }
        };
        if crate::value_contains_potentially_issued_ssn(&value) {
            tracing::warn!("refused potentially issued SSN-shaped visitor input");
            return ApiError::RealSsn.into_response();
        }
    }

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DemoClient {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) filing_status: String,
    pub(crate) w2_line: String,
    pub(crate) ssn: String,
    pub(crate) email: String,
    pub(crate) test_data: bool,
}

#[derive(Debug, Serialize)]
pub struct DemoClientView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub test_data: bool,
}

impl From<&DemoClient> for DemoClientView {
    fn from(client: &DemoClient) -> Self {
        Self {
            id: client.id.clone(),
            name: client.name.clone(),
            email: client.email.clone(),
            test_data: client.test_data,
        }
    }
}

fn demo_clients() -> Vec<DemoClient> {
    vec![
        DemoClient {
            id: "demo-avery".to_owned(),
            name: "Avery Morgan".to_owned(),
            filing_status: "Single".to_owned(),
            w2_line: "W-2 box 1 wages: $48,250; box 2 federal tax withheld: $5,410".to_owned(),
            ssn: pru_ssn::synthetic_test_ssn(1, pru_ssn::TestSsnRange::Group00),
            email: "avery.morgan@example.com".to_owned(),
            test_data: true,
        },
        DemoClient {
            id: "demo-riley".to_owned(),
            name: "Riley Chen".to_owned(),
            filing_status: "Married filing jointly".to_owned(),
            w2_line: "W-2 box 1 wages: $73,900; box 2 federal tax withheld: $8,120".to_owned(),
            ssn: pru_ssn::synthetic_test_ssn(2, pru_ssn::TestSsnRange::Area9xx),
            email: "riley.chen@example.com".to_owned(),
            test_data: true,
        },
    ]
}

async fn list_demo_clients(
    Extension(_workspace): Extension<WorkspaceContext>,
) -> Json<Vec<DemoClientView>> {
    Json(demo_clients().iter().map(DemoClientView::from).collect())
}

async fn reset_demo(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
) -> Result<Json<Vec<DemoClientView>>, ApiError> {
    Ok(Json(state.reset_demo(&workspace)?))
}

async fn chat(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
    AxumPath(client): AxumPath<String>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    Ok(Json(run_chat(&state, &workspace, &client, request).await?))
}

#[derive(Debug, Deserialize)]
pub struct MintConsentRequest {
    pub kind: ConsentKind,
    #[serde(default)]
    pub recipient: Option<String>,
    pub purpose: String,
    pub expires_at: DateTime<Utc>,
    pub pin: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct StoredConsent {
    pub(crate) id: Uuid,
    pub(crate) token: String,
    pub(crate) revocation_id: String,
    pub(crate) revoked: bool,
    pub(crate) record: ConsentRecord,
}

#[derive(Debug, Serialize)]
pub struct ConsentView {
    pub id: Uuid,
    pub kind: ConsentKind,
    pub recipient: Option<String>,
    pub purpose: String,
    pub signed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
}

impl From<&StoredConsent> for ConsentView {
    fn from(stored: &StoredConsent) -> Self {
        Self {
            id: stored.id,
            kind: stored.record.kind,
            recipient: stored.record.recipient.clone(),
            purpose: stored.record.purpose.clone(),
            signed_at: stored.record.signed_at,
            expires_at: stored.record.expires_at,
            revoked: stored.revoked,
        }
    }
}

async fn mint_consent(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
    AxumPath(client): AxumPath<String>,
    Json(request): Json<MintConsentRequest>,
) -> Result<(StatusCode, Json<ConsentView>), ApiError> {
    reject_potentially_issued_ssns(
        request
            .recipient
            .iter()
            .map(String::as_str)
            .chain([request.purpose.as_str(), request.pin.as_str()]),
    )?;
    state.ensure_demo_client(&client)?;
    let signed_at = Utc::now();
    let record = ConsentRecord::new(
        &client,
        request.kind,
        request.recipient,
        request.purpose,
        signed_at,
        "TEST DATA signer",
        &request.pin,
    )?
    .with_expiry(request.expires_at)?;
    let token = state.authority.mint(&record, &request.pin)?;
    let verified = state
        .authority
        .verify(&token, signed_at, &RevocationList::default())?;
    let stored = StoredConsent {
        id: Uuid::new_v4(),
        token,
        revocation_id: verified.revocation_id,
        revoked: false,
        record,
    };
    let view = ConsentView::from(&stored);
    with_consents(&state, &workspace, &client, |consents| {
        consents.push(stored);
    })?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn list_consents(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
    AxumPath(client): AxumPath<String>,
) -> Result<Json<Vec<ConsentView>>, ApiError> {
    state.ensure_demo_client(&client)?;
    let consents = read_consents(&state, &workspace, &client)?;
    Ok(Json(consents.iter().map(ConsentView::from).collect()))
}

async fn revoke_consent(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
    AxumPath((client, id)): AxumPath<(String, Uuid)>,
) -> Result<Json<ConsentView>, ApiError> {
    state.ensure_demo_client(&client)?;
    let mut revoked = None;
    with_consents(&state, &workspace, &client, |consents| {
        if let Some(consent) = consents.iter_mut().find(|consent| consent.id == id) {
            consent.revoked = true;
            revoked = Some(consent.clone());
        }
    })?;
    let consent = revoked.ok_or(ApiError::NotFound)?;
    state.gateway.revoke(consent.revocation_id.clone())?;
    Ok(Json(ConsentView::from(&consent)))
}

async fn get_ledger(
    State(state): State<ApiState>,
    Extension(workspace): Extension<WorkspaceContext>,
    AxumPath(client): AxumPath<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    state.ensure_demo_client(&client)?;
    Ok(Json(
        state
            .gateway
            .ledger_entries_in_workspace(&workspace.id(), &client)?,
    ))
}

fn client_directory(root: &Path, client: &str) -> Result<PathBuf, ApiError> {
    validate_client_id(client)?;
    Ok(root.join(client))
}

pub(crate) fn read_consents(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client: &str,
) -> Result<Vec<StoredConsent>, ApiError> {
    let _guard = state
        .storage_lock
        .lock()
        .map_err(|_| ApiError::Storage("consent storage lock poisoned".to_owned()))?;
    read_consents_unlocked(workspace.root(), client)
}

fn with_consents(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client: &str,
    update: impl FnOnce(&mut Vec<StoredConsent>),
) -> Result<(), ApiError> {
    let _guard = state
        .storage_lock
        .lock()
        .map_err(|_| ApiError::Storage("consent storage lock poisoned".to_owned()))?;
    let mut consents = read_consents_unlocked(workspace.root(), client)?;
    update(&mut consents);
    let directory = client_directory(workspace.root(), client)?;
    fs::create_dir_all(&directory)
        .map_err(|error| ApiError::Storage(format!("create {}: {error}", directory.display())))?;
    let path = directory.join("consents.json");
    write_json(&path, &consents)
}

fn read_consents_unlocked(root: &Path, client: &str) -> Result<Vec<StoredConsent>, ApiError> {
    let path = client_directory(root, client)?.join("consents.json");
    match fs::read(&path) {
        Ok(body) => serde_json::from_slice(&body)
            .map_err(|error| ApiError::Storage(format!("decode {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(ApiError::Storage(format!(
            "read {}: {error}",
            path.display()
        ))),
    }
}

pub(crate) fn read_demo_client(
    state: &ApiState,
    workspace: &WorkspaceContext,
    client: &str,
) -> Result<DemoClient, ApiError> {
    state.ensure_demo_client(client)?;
    let path = client_directory(workspace.root(), client)?.join("client.json");
    let body = fs::read(&path)
        .map_err(|error| ApiError::Storage(format!("read {}: {error}", path.display())))?;
    serde_json::from_slice(&body)
        .map_err(|error| ApiError::Storage(format!("decode {}: {error}", path.display())))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), ApiError> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|error| ApiError::Storage(format!("encode {}: {error}", path.display())))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, body)
        .map_err(|error| ApiError::Storage(format!("write {}: {error}", temporary.display())))?;
    fs::rename(&temporary, path).map_err(|error| {
        ApiError::Storage(format!(
            "rename {} to {}: {error}",
            temporary.display(),
            path.display()
        ))
    })
}

pub(crate) fn reject_potentially_issued_ssns<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), ApiError> {
    if values
        .into_iter()
        .any(pru_ssn::contains_potentially_issued_ssn)
    {
        tracing::warn!("refused potentially issued SSN-shaped visitor input");
        Err(ApiError::RealSsn)
    } else {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    #[error(transparent)]
    Consent(#[from] pru_consent::ConsentError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("consent not found")]
    NotFound,
    #[error("storage failed: {0}")]
    Storage(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("{REAL_SSN_REFUSAL_MESSAGE}")]
    RealSsn,
    #[error("request body exceeds {MAX_REQUEST_BODY_BYTES} bytes")]
    PayloadTooLarge,
    #[error("model response was invalid")]
    ModelProtocol,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Workspace(WorkspaceError::Capacity | WorkspaceError::Deleting) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Consent(_)
            | Self::Gateway(GatewayError::Consent(_))
            | Self::InvalidRequest(_)
            | Self::RealSsn
            | Self::Workspace(WorkspaceError::Invalid) => StatusCode::BAD_REQUEST,
            Self::Gateway(error) => return error.into_response(),
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Storage(_) | Self::Workspace(WorkspaceError::State | WorkspaceError::Storage) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::ModelProtocol => StatusCode::BAD_GATEWAY,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
