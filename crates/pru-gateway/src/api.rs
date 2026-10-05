use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use pru_consent::{ConsentAuthority, ConsentKind, ConsentRecord, RevocationList};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{GatewayError, GatewayState, validate_client_id};

#[derive(Clone)]
pub struct ApiState {
    gateway: GatewayState,
    authority: Arc<ConsentAuthority>,
    data_root: PathBuf,
    storage_lock: Arc<Mutex<()>>,
}

impl ApiState {
    #[must_use]
    pub fn new(gateway: GatewayState, authority: ConsentAuthority, data_root: PathBuf) -> Self {
        Self {
            gateway,
            authority: Arc::new(authority),
            data_root,
            storage_lock: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn gateway(&self) -> &GatewayState {
        &self.gateway
    }

    pub fn reset_demo(&self) -> Result<Vec<DemoClientView>, ApiError> {
        let _guard = self
            .storage_lock
            .lock()
            .map_err(|_| ApiError::Storage("demo storage lock poisoned".to_owned()))?;
        for client in demo_clients() {
            for consent in read_consents_unlocked(&self.data_root, &client.id)? {
                self.gateway.revoke(consent.revocation_id)?;
            }
            let directory = client_directory(&self.data_root, &client.id)?;
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

    fn ensure_demo_client(&self, client: &str) -> Result<(), ApiError> {
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
        .with_state(state)
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DemoClient {
    id: String,
    name: String,
    filing_status: String,
    w2_line: String,
    ssn: String,
    email: String,
    test_data: bool,
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

async fn list_demo_clients() -> Json<Vec<DemoClientView>> {
    Json(demo_clients().iter().map(DemoClientView::from).collect())
}

async fn reset_demo(State(state): State<ApiState>) -> Result<Json<Vec<DemoClientView>>, ApiError> {
    Ok(Json(state.reset_demo()?))
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
struct StoredConsent {
    id: Uuid,
    token: String,
    revocation_id: String,
    revoked: bool,
    record: ConsentRecord,
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
    AxumPath(client): AxumPath<String>,
    Json(request): Json<MintConsentRequest>,
) -> Result<(StatusCode, Json<ConsentView>), ApiError> {
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
    with_consents(&state, &client, |consents| consents.push(stored))?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn list_consents(
    State(state): State<ApiState>,
    AxumPath(client): AxumPath<String>,
) -> Result<Json<Vec<ConsentView>>, ApiError> {
    state.ensure_demo_client(&client)?;
    let consents = read_consents(&state, &client)?;
    Ok(Json(consents.iter().map(ConsentView::from).collect()))
}

async fn revoke_consent(
    State(state): State<ApiState>,
    AxumPath((client, id)): AxumPath<(String, Uuid)>,
) -> Result<Json<ConsentView>, ApiError> {
    state.ensure_demo_client(&client)?;
    let mut revoked = None;
    with_consents(&state, &client, |consents| {
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
    AxumPath(client): AxumPath<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    state.ensure_demo_client(&client)?;
    Ok(Json(state.gateway.ledger_entries(&client)?))
}

fn client_directory(root: &Path, client: &str) -> Result<PathBuf, ApiError> {
    validate_client_id(client)?;
    Ok(root.join(client))
}

fn read_consents(state: &ApiState, client: &str) -> Result<Vec<StoredConsent>, ApiError> {
    let _guard = state
        .storage_lock
        .lock()
        .map_err(|_| ApiError::Storage("consent storage lock poisoned".to_owned()))?;
    read_consents_unlocked(&state.data_root, client)
}

fn with_consents(
    state: &ApiState,
    client: &str,
    update: impl FnOnce(&mut Vec<StoredConsent>),
) -> Result<(), ApiError> {
    let _guard = state
        .storage_lock
        .lock()
        .map_err(|_| ApiError::Storage("consent storage lock poisoned".to_owned()))?;
    let mut consents = read_consents_unlocked(&state.data_root, client)?;
    update(&mut consents);
    let directory = client_directory(&state.data_root, client)?;
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

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    #[error(transparent)]
    Consent(#[from] pru_consent::ConsentError),
    #[error("consent not found")]
    NotFound,
    #[error("storage failed: {0}")]
    Storage(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Consent(_) | Self::Gateway(GatewayError::Consent(_)) => StatusCode::BAD_REQUEST,
            Self::Gateway(error) => return error.into_response(),
            Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
