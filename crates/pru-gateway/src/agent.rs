use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::workspace::WorkspaceContext;

pub const AGENT_TURN_QUEUE_WAIT_SECS: u64 = 30;
pub const AGENT_TURN_TTL_SECS: u64 = 90;
pub const MAX_AGENT_MODEL_CALLS: u8 = 6;
pub const MAX_AGENT_TOOL_CALLS: u8 = 6;
pub const AGENT_BUSY_MESSAGE: &str = "Pru's agent is finishing another visitor's turn.";

#[derive(Clone)]
pub struct AgentCredential {
    encoded: Arc<str>,
}

impl AgentCredential {
    #[must_use]
    pub fn generate() -> Self {
        let bytes: [u8; 32] = rand::random();
        Self {
            encoded: hex::encode(bytes).into(),
        }
    }

    pub fn from_hex(value: &str) -> Result<Self, AgentCredentialError> {
        let decoded = hex::decode(value).map_err(|_| AgentCredentialError)?;
        if decoded.len() != 32 {
            return Err(AgentCredentialError);
        }
        Ok(Self {
            encoded: value.to_ascii_lowercase().into(),
        })
    }

    #[must_use]
    pub fn matches_bearer(&self, value: &str) -> bool {
        let Some(candidate) = value.strip_prefix("Bearer ") else {
            return false;
        };
        constant_time_eq::constant_time_eq(self.encoded.as_bytes(), candidate.as_bytes())
    }
}

#[derive(Debug, Error)]
#[error("PRU_AGENT_SECRET must encode exactly 32 random bytes as 64 hexadecimal characters")]
pub struct AgentCredentialError;

#[derive(Clone)]
pub struct AgentTurnRegistry {
    inner: Arc<AgentTurnRegistryInner>,
}

struct AgentTurnRegistryInner {
    active: Mutex<Option<ActiveAgentTurn>>,
    slot: Arc<Semaphore>,
    queue_wait: Duration,
    turn_ttl: Duration,
}

struct ActiveAgentTurn {
    id: Uuid,
    workspace: WorkspaceContext,
    client_id: String,
    purpose: String,
    consent_token: String,
    deadline: Instant,
    model_calls: u8,
    tool_calls: u8,
}

#[derive(Clone)]
pub struct ActiveTurnSnapshot {
    pub turn_id: Uuid,
    pub workspace: WorkspaceContext,
    pub client_id: String,
    pub purpose: String,
    pub consent_token: String,
    pub model_calls: u8,
    pub tool_calls: u8,
}

pub struct AgentTurnLease {
    registry: AgentTurnRegistry,
    turn_id: Uuid,
    _permit: OwnedSemaphorePermit,
}

impl AgentTurnRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_durations(
            Duration::from_secs(AGENT_TURN_QUEUE_WAIT_SECS),
            Duration::from_secs(AGENT_TURN_TTL_SECS),
        )
    }

    fn with_durations(queue_wait: Duration, turn_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(AgentTurnRegistryInner {
                active: Mutex::new(None),
                slot: Arc::new(Semaphore::new(1)),
                queue_wait,
                turn_ttl,
            }),
        }
    }

    pub async fn begin(
        &self,
        workspace: WorkspaceContext,
        client_id: String,
        purpose: String,
        consent_token: String,
    ) -> Result<AgentTurnLease, AgentTurnError> {
        let permit = tokio::time::timeout(
            self.inner.queue_wait,
            Arc::clone(&self.inner.slot).acquire_owned(),
        )
        .await
        .map_err(|_| AgentTurnError::Busy)?
        .map_err(|_| AgentTurnError::State)?;
        let turn_id = Uuid::new_v4();
        let mut active = self
            .inner
            .active
            .lock()
            .map_err(|_| AgentTurnError::State)?;
        if active.is_some() {
            return Err(AgentTurnError::State);
        }
        *active = Some(ActiveAgentTurn {
            id: turn_id,
            workspace,
            client_id,
            purpose,
            consent_token,
            deadline: Instant::now() + self.inner.turn_ttl,
            model_calls: 0,
            tool_calls: 0,
        });
        drop(active);
        Ok(AgentTurnLease {
            registry: self.clone(),
            turn_id,
            _permit: permit,
        })
    }

    pub fn active(&self, expected: Uuid) -> Result<ActiveTurnSnapshot, AgentTurnError> {
        self.with_active(expected, |turn| Ok(snapshot(turn)))
    }

    pub fn reserve_model_call(&self, expected: Uuid) -> Result<ActiveTurnSnapshot, AgentTurnError> {
        self.with_active(expected, |turn| {
            if turn.model_calls >= MAX_AGENT_MODEL_CALLS {
                return Err(AgentTurnError::ModelCallLimit);
            }
            turn.model_calls += 1;
            Ok(snapshot(turn))
        })
    }

    pub fn reserve_tool_call(&self, expected: Uuid) -> Result<ActiveTurnSnapshot, AgentTurnError> {
        self.with_active(expected, |turn| {
            if turn.tool_calls >= MAX_AGENT_TOOL_CALLS {
                return Err(AgentTurnError::ToolCallLimit);
            }
            turn.tool_calls += 1;
            Ok(snapshot(turn))
        })
    }

    fn with_active<T>(
        &self,
        expected: Uuid,
        operation: impl FnOnce(&mut ActiveAgentTurn) -> Result<T, AgentTurnError>,
    ) -> Result<T, AgentTurnError> {
        let mut active = self
            .inner
            .active
            .lock()
            .map_err(|_| AgentTurnError::State)?;
        let turn = active.as_mut().ok_or(AgentTurnError::Missing)?;
        if turn.id != expected {
            return Err(AgentTurnError::Mismatch);
        }
        if Instant::now() >= turn.deadline {
            return Err(AgentTurnError::Expired);
        }
        operation(turn)
    }
}

impl Default for AgentTurnRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentTurnLease {
    #[must_use]
    pub fn turn_id(&self) -> Uuid {
        self.turn_id
    }
}

impl Drop for AgentTurnLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.inner.active.lock()
            && active.as_ref().is_some_and(|turn| turn.id == self.turn_id)
        {
            *active = None;
        }
    }
}

fn snapshot(turn: &ActiveAgentTurn) -> ActiveTurnSnapshot {
    ActiveTurnSnapshot {
        turn_id: turn.id,
        workspace: turn.workspace.clone(),
        client_id: turn.client_id.clone(),
        purpose: turn.purpose.clone(),
        consent_token: turn.consent_token.clone(),
        model_calls: turn.model_calls,
        tool_calls: turn.tool_calls,
    }
}

#[derive(Debug, Error)]
pub enum AgentTurnError {
    #[error("{AGENT_BUSY_MESSAGE}")]
    Busy,
    #[error("no Hermes turn is active")]
    Missing,
    #[error("the Hermes turn id does not match the active turn")]
    Mismatch,
    #[error("the Hermes turn expired after {AGENT_TURN_TTL_SECS} seconds")]
    Expired,
    #[error("the Hermes turn exceeded {MAX_AGENT_MODEL_CALLS} model calls")]
    ModelCallLimit,
    #[error("the Hermes turn exceeded {MAX_AGENT_TOOL_CALLS} tool calls")]
    ToolCallLimit,
    #[error("agent turn state is unavailable")]
    State,
}

impl IntoResponse for AgentTurnError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Busy | Self::ModelCallLimit | Self::ToolCallLimit => {
                StatusCode::TOO_MANY_REQUESTS
            }
            Self::Missing | Self::Mismatch | Self::Expired => StatusCode::CONFLICT,
            Self::State => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.to_string()}))).into_response()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LedgerContext {
    pub turn_id: Uuid,
    pub actor: String,
    pub step: String,
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::workspace::WorkspaceManager;

    const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

    async fn workspace() -> (TempDir, WorkspaceContext) {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::new(temp.path().to_path_buf());
        let (workspace, _) = manager.acquire(WORKSPACE, false).await.expect("workspace");
        (temp, workspace)
    }

    #[test]
    fn credential_requires_32_bytes_and_compares_the_bearer() {
        let secret = "ab".repeat(32);
        let credential = AgentCredential::from_hex(&secret).expect("credential");
        assert!(credential.matches_bearer(&format!("Bearer {secret}")));
        assert!(!credential.matches_bearer(&format!("Bearer {}", "ac".repeat(32))));
        assert!(!credential.matches_bearer("Bearer ab"));
        assert!(!credential.matches_bearer(&secret));
        assert!(AgentCredential::from_hex("ab").is_err());
    }

    #[tokio::test]
    async fn a_second_turn_waits_until_the_first_releases() {
        let registry =
            AgentTurnRegistry::with_durations(Duration::from_secs(1), Duration::from_secs(1));
        let (_first_temp, first_workspace) = workspace().await;
        let (_second_temp, second_workspace) = workspace().await;
        let first = registry
            .begin(
                first_workspace,
                "a".into(),
                "purpose".into(),
                "token".into(),
            )
            .await
            .expect("first turn");
        let waiting_registry = registry.clone();
        let waiter = tokio::spawn(async move {
            waiting_registry
                .begin(
                    second_workspace,
                    "b".into(),
                    "purpose".into(),
                    "token".into(),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(first);
        assert!(waiter.await.expect("waiter task").is_ok());
    }

    #[tokio::test]
    async fn a_busy_turn_times_out_with_429() {
        let registry =
            AgentTurnRegistry::with_durations(Duration::from_millis(20), Duration::from_secs(1));
        let (_first_temp, first_workspace) = workspace().await;
        let (_second_temp, second_workspace) = workspace().await;
        let _first = registry
            .begin(
                first_workspace,
                "a".into(),
                "purpose".into(),
                "token".into(),
            )
            .await
            .expect("first turn");
        let error = match registry
            .begin(
                second_workspace,
                "b".into(),
                "purpose".into(),
                "token".into(),
            )
            .await
        {
            Ok(_) => panic!("busy turn was admitted"),
            Err(error) => error,
        };
        assert_eq!(
            error.into_response().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn seventh_calls_are_refused() {
        let registry =
            AgentTurnRegistry::with_durations(Duration::from_secs(1), Duration::from_secs(1));
        let (_temp, workspace) = workspace().await;
        let turn = registry
            .begin(workspace, "a".into(), "purpose".into(), "token".into())
            .await
            .expect("turn");
        for _ in 0..MAX_AGENT_MODEL_CALLS {
            registry
                .reserve_model_call(turn.turn_id())
                .expect("model call inside cap");
        }
        assert!(matches!(
            registry.reserve_model_call(turn.turn_id()),
            Err(AgentTurnError::ModelCallLimit)
        ));
        for _ in 0..MAX_AGENT_TOOL_CALLS {
            registry
                .reserve_tool_call(turn.turn_id())
                .expect("tool call inside cap");
        }
        assert!(matches!(
            registry.reserve_tool_call(turn.turn_id()),
            Err(AgentTurnError::ToolCallLimit)
        ));
    }

    #[tokio::test]
    async fn turn_expires_after_the_named_ttl() {
        let registry =
            AgentTurnRegistry::with_durations(Duration::from_secs(1), Duration::from_millis(20));
        let (_temp, workspace) = workspace().await;
        let turn = registry
            .begin(workspace, "a".into(), "purpose".into(), "token".into())
            .await
            .expect("turn");
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(matches!(
            registry.active(turn.turn_id()),
            Err(AgentTurnError::Expired)
        ));
    }
}
