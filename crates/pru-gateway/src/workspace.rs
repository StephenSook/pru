use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

pub const WORKSPACE_HEADER: &str = "x-pru-workspace-id";
pub const WORKSPACE_IDLE_TTL_SECS: u64 = 3_600;
pub const UNTOUCHED_WORKSPACE_IDLE_TTL_SECS: u64 = 600;
pub const WORKSPACE_CLEANUP_INTERVAL_SECS: u64 = 60;
pub const MAX_CONCURRENT_WORKSPACES: usize = 200;
const LAST_ACCESS_FILE: &str = ".last_access";
const TOUCHED_FILE: &str = ".touched";

#[derive(Clone)]
pub struct WorkspaceManager {
    inner: Arc<WorkspaceManagerInner>,
}

struct WorkspaceManagerInner {
    root: PathBuf,
    idle_ttl: Duration,
    untouched_idle_ttl: Duration,
    maximum: usize,
    entries: Mutex<WorkspaceEntries>,
}

#[derive(Default)]
struct WorkspaceEntries {
    active: HashMap<Uuid, WorkspaceEntry>,
    deleting: HashMap<Uuid, Arc<RwLock<()>>>,
}

struct WorkspaceEntry {
    last_access: Instant,
    gate: Arc<RwLock<()>>,
    // Requests that found this entry but do not hold its lease yet. Changed only under the
    // registry mutex, so eviction and cleanup, which also hold it, never take such an entry.
    pending: usize,
}

/// Counts a request as pending on a workspace until it holds the lease, including when the
/// request is cancelled while waiting.
struct PendingLease {
    inner: Arc<WorkspaceManagerInner>,
    id: Uuid,
}

impl Drop for PendingLease {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.inner.entries.lock()
            && let Some(entry) = entries.active.get_mut(&self.id)
        {
            entry.pending = entry.pending.saturating_sub(1);
        }
    }
}

#[derive(Clone)]
pub struct WorkspaceContext {
    id: Uuid,
    root: PathBuf,
    _lease: Arc<WorkspaceLease>,
}

enum WorkspaceLease {
    Read { _guard: OwnedRwLockReadGuard<()> },
    Write { _guard: OwnedRwLockWriteGuard<()> },
}

impl WorkspaceContext {
    #[must_use]
    pub fn id(&self) -> String {
        self.id.to_string()
    }

    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn mark_touched(&self) -> Result<(), WorkspaceError> {
        fs::write(self.root.join(TOUCHED_FILE), []).map_err(|_| WorkspaceError::Storage)
    }
}

impl WorkspaceManager {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self::with_limits(
            root,
            Duration::from_secs(WORKSPACE_IDLE_TTL_SECS),
            Duration::from_secs(UNTOUCHED_WORKSPACE_IDLE_TTL_SECS),
            MAX_CONCURRENT_WORKSPACES,
        )
    }

    fn with_limits(
        root: PathBuf,
        idle_ttl: Duration,
        untouched_idle_ttl: Duration,
        maximum: usize,
    ) -> Self {
        Self {
            inner: Arc::new(WorkspaceManagerInner {
                root,
                idle_ttl,
                untouched_idle_ttl,
                maximum,
                entries: Mutex::new(WorkspaceEntries::default()),
            }),
        }
    }

    pub async fn acquire(
        &self,
        raw_id: &str,
        exclusive: bool,
    ) -> Result<(WorkspaceContext, bool), WorkspaceError> {
        let id = parse_workspace_id(raw_id)?;
        self.cleanup_expired().await;
        let (gate, is_new) = loop {
            {
                let mut entries = self
                    .inner
                    .entries
                    .lock()
                    .map_err(|_| WorkspaceError::State)?;
                if entries.deleting.contains_key(&id) {
                    return Err(WorkspaceError::Deleting);
                }
                if let Some(entry) = entries.active.get_mut(&id) {
                    entry.last_access = Instant::now();
                    entry.pending += 1;
                    break (Arc::clone(&entry.gate), false);
                }
                if entries.active.len() + entries.deleting.len() < self.inner.maximum {
                    let gate = Arc::new(RwLock::new(()));
                    entries.active.insert(
                        id,
                        WorkspaceEntry {
                            last_access: Instant::now(),
                            gate: Arc::clone(&gate),
                            pending: 1,
                        },
                    );
                    break (gate, true);
                }
            }
            if !self.evict_lru_untouched().await? {
                return Err(WorkspaceError::Capacity);
            }
        };
        let pending = PendingLease {
            inner: Arc::clone(&self.inner),
            id,
        };
        let lease = if exclusive || is_new {
            WorkspaceLease::Write {
                _guard: gate.write_owned().await,
            }
        } else {
            WorkspaceLease::Read {
                _guard: gate.read_owned().await,
            }
        };
        // The lease now holds the gate, so eviction's try_write fails from here on.
        drop(pending);
        let root = self.inner.root.join(id.to_string());
        if touch_workspace(&root).is_err() {
            if is_new && let Ok(mut entries) = self.inner.entries.lock() {
                entries.active.remove(&id);
            }
            return Err(WorkspaceError::Storage);
        }
        Ok((
            WorkspaceContext {
                id,
                root,
                _lease: Arc::new(lease),
            },
            is_new,
        ))
    }

    async fn evict_lru_untouched(&self) -> Result<bool, WorkspaceError> {
        let selected = {
            let mut entries = self
                .inner
                .entries
                .lock()
                .map_err(|_| WorkspaceError::State)?;
            let mut candidates = entries
                .active
                .iter()
                .filter_map(|(id, entry)| {
                    let root = self.inner.root.join(id.to_string());
                    (entry.pending == 0 && !workspace_is_touched(&root))
                        .then_some((*id, entry.last_access))
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|(_, last_access)| *last_access);

            let mut selected = None;
            for (id, _) in candidates {
                let Some(gate) = entries.active.get(&id).map(|entry| Arc::clone(&entry.gate))
                else {
                    continue;
                };
                let Ok(exclusive) = gate.try_write_owned() else {
                    continue;
                };
                // A request that held the lease during the scan may have touched it and left.
                if workspace_is_touched(&self.inner.root.join(id.to_string())) {
                    continue;
                }
                let entry = entries
                    .active
                    .remove(&id)
                    .expect("selected workspace remains active");
                entries.deleting.insert(id, Arc::clone(&entry.gate));
                selected = Some((id, entry, exclusive));
                break;
            }
            selected
        };

        let Some((id, entry, _exclusive)) = selected else {
            return Ok(false);
        };
        let path = self.inner.root.join(id.to_string());
        let deletion = fs::remove_dir_all(&path);
        let deleted = match &deletion {
            Ok(()) => true,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        };
        let mut entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| WorkspaceError::State)?;
        entries.deleting.remove(&id);
        if deleted {
            Ok(true)
        } else {
            entries.active.insert(id, entry);
            Err(WorkspaceError::Storage)
        }
    }

    pub async fn cleanup_expired(&self) {
        let candidates = {
            let Ok(mut entries) = self.inner.entries.lock() else {
                tracing::error!("workspace registry lock poisoned during cleanup");
                return;
            };
            let now = Instant::now();
            let expired = entries
                .active
                .iter()
                .filter_map(|(id, entry)| {
                    let root = self.inner.root.join(id.to_string());
                    let idle_ttl = if workspace_is_touched(&root) {
                        self.inner.idle_ttl
                    } else {
                        self.inner.untouched_idle_ttl
                    };
                    (entry.pending == 0 && now.duration_since(entry.last_access) >= idle_ttl)
                        .then_some(*id)
                })
                .collect::<Vec<_>>();
            for id in &expired {
                if let Some(entry) = entries.active.remove(id) {
                    entries.deleting.insert(*id, entry.gate);
                }
            }
            expired
        };

        for id in candidates {
            let gate = {
                let Ok(entries) = self.inner.entries.lock() else {
                    tracing::error!("workspace registry lock poisoned during deletion");
                    return;
                };
                entries.deleting.get(&id).cloned()
            };
            let Some(gate) = gate else {
                continue;
            };
            let _exclusive = gate.write_owned().await;
            let path = self.inner.root.join(id.to_string());
            let deleted = match fs::remove_dir_all(&path) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => {
                    tracing::error!(error = %error, "failed to delete an expired workspace");
                    false
                }
            };
            if deleted && let Ok(mut entries) = self.inner.entries.lock() {
                entries.deleting.remove(&id);
            }
        }
        self.cleanup_orphans().await;
    }

    async fn cleanup_orphans(&self) {
        let Ok(directories) = fs::read_dir(&self.inner.root) else {
            return;
        };
        for directory in directories.flatten() {
            let path = directory.path();
            if !path.is_dir() {
                continue;
            }
            let Some(raw_id) = directory.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(id) = parse_workspace_id(&raw_id) else {
                continue;
            };
            if !workspace_is_expired(&path, self.inner.idle_ttl, self.inner.untouched_idle_ttl) {
                continue;
            }
            let gate = {
                let Ok(mut entries) = self.inner.entries.lock() else {
                    tracing::error!("workspace registry lock poisoned during orphan cleanup");
                    return;
                };
                if entries.active.contains_key(&id) || entries.deleting.contains_key(&id) {
                    continue;
                }
                let gate = Arc::new(RwLock::new(()));
                entries.deleting.insert(id, Arc::clone(&gate));
                gate
            };
            let _exclusive = gate.write_owned().await;
            let deleted = match fs::remove_dir_all(&path) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => {
                    tracing::error!(error = %error, "failed to delete an expired orphan workspace");
                    false
                }
            };
            if deleted && let Ok(mut entries) = self.inner.entries.lock() {
                entries.deleting.remove(&id);
            }
        }
    }

    pub async fn abandon_new(&self, context: &WorkspaceContext) {
        if let Ok(mut entries) = self.inner.entries.lock() {
            entries.active.remove(&context.id);
        }
        if let Err(error) = fs::remove_dir_all(&context.root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!(error = %error, "failed to remove an unseeded workspace");
        }
    }

    pub async fn cleanup_loop(self) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(WORKSPACE_CLEANUP_INTERVAL_SECS));
        loop {
            interval.tick().await;
            self.cleanup_expired().await;
        }
    }
}

fn touch_workspace(root: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    fs::write(root.join(LAST_ACCESS_FILE), [])
}

fn workspace_is_touched(root: &std::path::Path) -> bool {
    root.join(TOUCHED_FILE).is_file()
}

fn workspace_is_expired(
    root: &std::path::Path,
    idle_ttl: Duration,
    untouched_idle_ttl: Duration,
) -> bool {
    let marker = root.join(LAST_ACCESS_FILE);
    let metadata = fs::metadata(&marker).or_else(|_| fs::metadata(root));
    let Ok(modified) = metadata.and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let idle_ttl = if workspace_is_touched(root) {
        idle_ttl
    } else {
        untouched_idle_ttl
    };
    modified.elapsed().unwrap_or_default() >= idle_ttl
}

fn parse_workspace_id(raw: &str) -> Result<Uuid, WorkspaceError> {
    let id = Uuid::parse_str(raw).map_err(|_| WorkspaceError::Invalid)?;
    if id.to_string() == raw {
        Ok(id)
    } else {
        Err(WorkspaceError::Invalid)
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace cookie is missing or invalid")]
    Invalid,
    #[error("The public demo is at its 200-workspace limit. Please try again later.")]
    Capacity,
    #[error(
        "This workspace is being deleted after its idle timeout. Please reload for a new workspace."
    )]
    Deleting,
    #[error("workspace registry is unavailable")]
    State,
    #[error("workspace storage is unavailable")]
    Storage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";

    #[tokio::test]
    async fn refuses_a_workspace_beyond_the_named_capacity() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::from_secs(3_600),
            Duration::from_secs(600),
            2,
        );
        let (a, _) = manager.acquire(A, false).await.expect("workspace A");
        a.mark_touched().expect("touch workspace A");
        drop(a);
        let (b, _) = manager.acquire(B, false).await.expect("workspace B");
        b.mark_touched().expect("touch workspace B");
        drop(b);
        let error = manager
            .acquire(C, false)
            .await
            .err()
            .expect("capacity error");
        assert!(matches!(error, WorkspaceError::Capacity));
    }

    #[tokio::test]
    async fn evicts_the_least_recently_used_untouched_workspace_at_capacity() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::from_secs(3_600),
            Duration::from_secs(600),
            2,
        );
        let (a, _) = manager.acquire(A, false).await.expect("workspace A");
        drop(a);
        let (b, _) = manager.acquire(B, false).await.expect("workspace B");
        drop(b);
        {
            let mut entries = manager.inner.entries.lock().expect("workspace entries");
            entries
                .active
                .get_mut(&Uuid::parse_str(A).unwrap())
                .unwrap()
                .last_access = Instant::now() - Duration::from_secs(2);
        }

        let (c, is_new) = manager.acquire(C, false).await.expect("workspace C");

        assert!(is_new);
        assert!(!temp.path().join(A).exists());
        assert!(temp.path().join(B).exists());
        assert!(c.root().exists());
    }

    #[tokio::test]
    async fn touched_workspace_is_never_evicted_early() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::from_secs(3_600),
            Duration::from_secs(600),
            2,
        );
        let (a, _) = manager.acquire(A, false).await.expect("workspace A");
        a.mark_touched().expect("touch workspace A");
        drop(a);
        let (b, _) = manager.acquire(B, false).await.expect("workspace B");
        drop(b);
        {
            let mut entries = manager.inner.entries.lock().expect("workspace entries");
            entries
                .active
                .get_mut(&Uuid::parse_str(A).unwrap())
                .unwrap()
                .last_access = Instant::now() - Duration::from_secs(2);
        }

        let (c, _) = manager.acquire(C, false).await.expect("workspace C");

        assert!(temp.path().join(A).exists());
        assert!(!temp.path().join(B).exists());
        assert!(c.root().exists());
    }

    // A request that has found its workspace but not yet taken the lease leaves the gate free.
    // Eviction must not take that workspace in the gap.
    #[tokio::test]
    async fn a_workspace_waiting_for_its_lease_is_never_evicted() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::from_secs(3_600),
            Duration::from_secs(600),
            2,
        );
        let (a, _) = manager.acquire(A, false).await.expect("workspace A");
        drop(a);
        let (b, _) = manager.acquire(B, false).await.expect("workspace B");
        drop(b);
        {
            let mut entries = manager.inner.entries.lock().expect("workspace entries");
            let a = entries
                .active
                .get_mut(&Uuid::parse_str(A).unwrap())
                .unwrap();
            a.last_access = Instant::now() - Duration::from_secs(2);
            a.pending = 1;
        }

        let (c, _) = manager.acquire(C, false).await.expect("workspace C");

        assert!(temp.path().join(A).exists());
        assert!(!temp.path().join(B).exists());
        assert!(c.root().exists());
    }

    #[tokio::test]
    async fn the_pending_count_returns_to_zero_once_the_lease_is_held() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::new(temp.path().to_path_buf());
        // A new workspace takes the write lease, so release it before reading again.
        let (a, _) = manager.acquire(A, false).await.expect("workspace A");
        drop(a);
        let (first, _) = manager.acquire(A, false).await.expect("first reader");
        let (second, _) = manager.acquire(A, false).await.expect("second reader");
        let entries = manager.inner.entries.lock().expect("workspace entries");
        assert_eq!(entries.active[&Uuid::parse_str(A).unwrap()].pending, 0);
        drop(entries);
        drop((first, second));
    }

    #[tokio::test]
    async fn cleanup_deletes_an_idle_workspace_directory() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::from_secs(3_600),
            Duration::from_secs(1),
            MAX_CONCURRENT_WORKSPACES,
        );
        let (workspace, _) = manager.acquire(A, false).await.expect("workspace");
        fs::create_dir_all(workspace.root()).expect("workspace directory");
        fs::write(workspace.root().join("marker"), b"test").expect("marker");
        drop(workspace);
        {
            let mut entries = manager.inner.entries.lock().expect("workspace entries");
            entries
                .active
                .get_mut(&Uuid::parse_str(A).unwrap())
                .unwrap()
                .last_access = Instant::now() - Duration::from_secs(2);
        }

        manager.cleanup_expired().await;

        assert!(!temp.path().join(A).exists());
    }

    #[tokio::test]
    async fn cleanup_deletes_expired_workspace_left_by_an_earlier_process() {
        let temp = TempDir::new().expect("temp directory");
        let root = temp.path().join(A);
        touch_workspace(&root).expect("orphan workspace marker");
        let manager = WorkspaceManager::with_limits(
            temp.path().to_path_buf(),
            Duration::ZERO,
            Duration::ZERO,
            MAX_CONCURRENT_WORKSPACES,
        );

        manager.cleanup_expired().await;

        assert!(!root.exists());
    }

    #[tokio::test]
    async fn rejects_noncanonical_workspace_ids_before_path_use() {
        let temp = TempDir::new().expect("temp directory");
        let manager = WorkspaceManager::new(temp.path().to_path_buf());
        for invalid in ["", "../escape", "11111111111141118111111111111111"] {
            assert!(matches!(
                manager.acquire(invalid, false).await,
                Err(WorkspaceError::Invalid)
            ));
        }
    }

    #[test]
    fn public_workspace_constants_are_fixed() {
        assert_eq!(WORKSPACE_IDLE_TTL_SECS, 3_600);
        assert_eq!(UNTOUCHED_WORKSPACE_IDLE_TTL_SECS, 600);
        assert_eq!(WORKSPACE_CLEANUP_INTERVAL_SECS, 60);
        assert_eq!(MAX_CONCURRENT_WORKSPACES, 200);
    }
}
