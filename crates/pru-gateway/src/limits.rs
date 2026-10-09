use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RATE_LIMIT_WINDOW_SECS: u64 = 60;
pub const CLIENT_IP_HEADER: &str = "x-pru-client-ip";
pub const MAX_REQUESTS_PER_IP_PER_WINDOW: usize = 120;
pub const MAX_REQUESTS_PER_WORKSPACE_PER_WINDOW: usize = 30;
pub const DAILY_TOKEN_FACTORY_TOKEN_BUDGET: u64 = 250_000;
pub const TOKEN_FACTORY_CALL_RESERVATION_TOKENS: u64 = 32_000;

pub const IP_RATE_LIMIT_MESSAGE: &str =
    "This public demo has received too many requests from this address. Please wait a minute.";
pub const WORKSPACE_RATE_LIMIT_MESSAGE: &str =
    "This public demo workspace has received too many requests. Please wait a minute.";
pub const TOKEN_BUDGET_MESSAGE: &str =
    "The public demo's daily hosted-model token budget has been spent. Try again tomorrow.";

#[derive(Clone)]
pub struct RequestRateLimiter {
    inner: Arc<Mutex<RateState>>,
    window: Duration,
    ip_maximum: usize,
    workspace_maximum: usize,
}

#[derive(Default)]
struct RateState {
    ips: HashMap<String, RateCounter>,
    workspaces: HashMap<String, RateCounter>,
}

struct RateCounter {
    started: Instant,
    count: usize,
}

impl RequestRateLimiter {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(
            Duration::from_secs(RATE_LIMIT_WINDOW_SECS),
            MAX_REQUESTS_PER_IP_PER_WINDOW,
            MAX_REQUESTS_PER_WORKSPACE_PER_WINDOW,
        )
    }

    fn with_limits(window: Duration, ip_maximum: usize, workspace_maximum: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RateState::default())),
            window,
            ip_maximum,
            workspace_maximum,
        }
    }

    pub fn check(&self, ip: &str, workspace: &str) -> Result<(), LimitError> {
        self.check_at(ip, workspace, Instant::now())
    }

    fn check_at(&self, ip: &str, workspace: &str, now: Instant) -> Result<(), LimitError> {
        let mut state = self.inner.lock().map_err(|_| LimitError::State)?;
        state
            .ips
            .retain(|_, counter| now.duration_since(counter.started) < self.window);
        state
            .workspaces
            .retain(|_, counter| now.duration_since(counter.started) < self.window);

        if current_count(&state.workspaces, workspace) >= self.workspace_maximum {
            return Err(LimitError::WorkspaceRate);
        }
        if current_count(&state.ips, ip) >= self.ip_maximum {
            return Err(LimitError::IpRate);
        }
        increment(&mut state.workspaces, workspace, now);
        increment(&mut state.ips, ip, now);
        Ok(())
    }
}

impl Default for RequestRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

fn current_count(counters: &HashMap<String, RateCounter>, key: &str) -> usize {
    counters.get(key).map_or(0, |counter| counter.count)
}

fn increment(counters: &mut HashMap<String, RateCounter>, key: &str, now: Instant) {
    counters
        .entry(key.to_owned())
        .and_modify(|counter| counter.count = counter.count.saturating_add(1))
        .or_insert(RateCounter {
            started: now,
            count: 1,
        });
}

#[derive(Clone)]
pub struct TokenBudget {
    inner: Arc<TokenBudgetInner>,
}

struct TokenBudgetInner {
    path: Option<PathBuf>,
    maximum: u64,
    reservation: u64,
    state: Mutex<TokenBudgetState>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct TokenBudgetState {
    day: NaiveDate,
    charged_tokens: u64,
}

pub struct TokenReservation {
    budget: TokenBudget,
    reserved: u64,
}

impl TokenBudget {
    pub fn new(root: Option<&Path>, now: DateTime<Utc>) -> Result<Self, LimitError> {
        let path = root.map(|root| root.join(".instance").join("token-budget.json"));
        Self::with_limits(
            path,
            now,
            DAILY_TOKEN_FACTORY_TOKEN_BUDGET,
            TOKEN_FACTORY_CALL_RESERVATION_TOKENS,
        )
    }

    fn with_limits(
        path: Option<PathBuf>,
        now: DateTime<Utc>,
        maximum: u64,
        reservation: u64,
    ) -> Result<Self, LimitError> {
        let state = load_state(path.as_deref(), now.date_naive())?;
        Ok(Self {
            inner: Arc::new(TokenBudgetInner {
                path,
                maximum,
                reservation,
                state: Mutex::new(state),
            }),
        })
    }

    pub fn reserve(&self, now: DateTime<Utc>) -> Result<TokenReservation, LimitError> {
        let mut state = self.inner.state.lock().map_err(|_| LimitError::State)?;
        reset_day(&mut state, now.date_naive());
        if state.charged_tokens.saturating_add(self.inner.reservation) > self.inner.maximum {
            return Err(LimitError::TokenBudget);
        }
        state.charged_tokens = state.charged_tokens.saturating_add(self.inner.reservation);
        persist_state(self.inner.path.as_deref(), &state)?;
        Ok(TokenReservation {
            budget: self.clone(),
            reserved: self.inner.reservation,
        })
    }
}

impl TokenReservation {
    pub fn commit(self, now: DateTime<Utc>, actual_tokens: u64) -> Result<(), LimitError> {
        let mut state = self
            .budget
            .inner
            .state
            .lock()
            .map_err(|_| LimitError::State)?;
        reset_day(&mut state, now.date_naive());
        state.charged_tokens = state
            .charged_tokens
            .saturating_sub(self.reserved)
            .saturating_add(actual_tokens);
        persist_state(self.budget.inner.path.as_deref(), &state)
    }
}

fn reset_day(state: &mut TokenBudgetState, day: NaiveDate) {
    if state.day != day {
        state.day = day;
        state.charged_tokens = 0;
    }
}

fn load_state(path: Option<&Path>, day: NaiveDate) -> Result<TokenBudgetState, LimitError> {
    let default = TokenBudgetState {
        day,
        charged_tokens: 0,
    };
    let Some(path) = path else {
        return Ok(default);
    };
    match fs::read(path) {
        Ok(body) => {
            let mut state = serde_json::from_slice::<TokenBudgetState>(&body)
                .map_err(|_| LimitError::Storage)?;
            reset_day(&mut state, day);
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(_) => Err(LimitError::Storage),
    }
}

fn persist_state(path: Option<&Path>, state: &TokenBudgetState) -> Result<(), LimitError> {
    let Some(path) = path else {
        return Ok(());
    };
    let parent = path.parent().ok_or(LimitError::Storage)?;
    fs::create_dir_all(parent).map_err(|_| LimitError::Storage)?;
    let body = serde_json::to_vec(state).map_err(|_| LimitError::Storage)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, body).map_err(|_| LimitError::Storage)?;
    fs::rename(&temporary, path).map_err(|_| LimitError::Storage)
}

#[derive(Debug, Error)]
pub enum LimitError {
    #[error("{IP_RATE_LIMIT_MESSAGE}")]
    IpRate,
    #[error("{WORKSPACE_RATE_LIMIT_MESSAGE}")]
    WorkspaceRate,
    #[error("{TOKEN_BUDGET_MESSAGE}")]
    TokenBudget,
    #[error("limit state is unavailable")]
    State,
    #[error("token budget storage is unavailable")]
    Storage,
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn per_workspace_rate_refuses_the_next_request_until_the_window_resets() {
        let limiter = RequestRateLimiter::with_limits(Duration::from_secs(60), 10, 2);
        let started = Instant::now();
        assert!(
            limiter
                .check_at("192.0.2.1", "workspace-a", started)
                .is_ok()
        );
        assert!(
            limiter
                .check_at("192.0.2.1", "workspace-a", started)
                .is_ok()
        );
        assert!(matches!(
            limiter.check_at("192.0.2.1", "workspace-a", started),
            Err(LimitError::WorkspaceRate)
        ));
        assert!(
            limiter
                .check_at(
                    "192.0.2.1",
                    "workspace-a",
                    started + Duration::from_secs(60)
                )
                .is_ok()
        );
    }

    #[test]
    fn per_ip_rate_combines_distinct_workspaces() {
        let limiter = RequestRateLimiter::with_limits(Duration::from_secs(60), 2, 10);
        let now = Instant::now();
        assert!(limiter.check_at("192.0.2.1", "workspace-a", now).is_ok());
        assert!(limiter.check_at("192.0.2.1", "workspace-b", now).is_ok());
        assert!(matches!(
            limiter.check_at("192.0.2.1", "workspace-c", now),
            Err(LimitError::IpRate)
        ));
    }

    #[test]
    fn daily_budget_reserves_concurrent_capacity_and_persists_charges() {
        let temp = TempDir::new().expect("temp directory");
        let path = temp.path().join("token-budget.json");
        let now = Utc::now();
        let budget =
            TokenBudget::with_limits(Some(path.clone()), now, 100, 60).expect("token budget");
        let first = budget.reserve(now).expect("first reservation");
        assert!(matches!(budget.reserve(now), Err(LimitError::TokenBudget)));
        first.commit(now, 20).expect("actual token charge");
        let _second = budget.reserve(now).expect("capacity released after commit");

        let reloaded =
            TokenBudget::with_limits(Some(path), now, 100, 60).expect("reloaded token budget");
        assert!(matches!(
            reloaded.reserve(now),
            Err(LimitError::TokenBudget)
        ));
    }

    #[test]
    fn daily_budget_resets_on_the_next_utc_day() {
        let now = Utc::now();
        let budget = TokenBudget::with_limits(None, now, 60, 60).expect("token budget");
        let _reservation = budget.reserve(now).expect("first day reservation");
        assert!(budget.reserve(now + TimeDelta::days(1)).is_ok());
    }

    #[test]
    fn public_limit_constants_are_fixed() {
        assert_eq!(RATE_LIMIT_WINDOW_SECS, 60);
        assert_eq!(MAX_REQUESTS_PER_IP_PER_WINDOW, 120);
        assert_eq!(MAX_REQUESTS_PER_WORKSPACE_PER_WINDOW, 30);
        assert_eq!(DAILY_TOKEN_FACTORY_TOKEN_BUDGET, 250_000);
        assert_eq!(TOKEN_FACTORY_CALL_RESERVATION_TOKENS, 32_000);
    }
}
