//! Shared server state: the axum `AppState` handle plus the activity and
//! shutdown primitives it owns.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::Config;
use crate::events::EventEmitter;
use crate::history;
use crate::pr::{GithubPrUrl, PrPhase, PrReviewState};
use crate::sse::EventBus;
use crate::state::{
    DemoScenarioLink, File, FileId, FileKind, SharedState, Source, State, ThreadId, default_file_id,
};
use crate::verdict::VerdictConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrExecutionMode {
    AgentEvent,
    DirectGh,
    DemoLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalizationStatus {
    InProgress,
    Failed,
    Complete,
}

#[derive(Clone, Debug)]
struct FinalizationClaim {
    owner: String,
    status: FinalizationStatus,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub state: SharedState,
    pub bus: Arc<EventBus>,
    pub emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    pub(super) source: Arc<std::sync::RwLock<Source>>,
    pub(super) file_bytes: Arc<HashMap<FileId, (Vec<u8>, &'static str)>>,
    file_versions: Arc<HashMap<FileId, String>>,
    pub(super) source_path: Arc<Option<PathBuf>>,
    pub(super) history_dir: Arc<PathBuf>,
    no_save: Arc<AtomicBool>,
    done_started: Arc<AtomicBool>,
    finalization: Arc<Mutex<Option<FinalizationClaim>>>,
    completion_scope: Arc<String>,
    pub(super) shutdown: ShutdownSignal,
    pub(super) activity: ActivityTracker,
    idle_timeout_secs: Arc<AtomicU64>,
    pub(super) verdict_config: Arc<Option<VerdictConfig>>,
    live_frame_url: Arc<Option<String>>,
    demo_scenarios: Arc<Option<Vec<DemoScenarioLink>>>,
    offline_demo: Arc<AtomicBool>,
    pub(crate) pr_review: Option<Arc<std::sync::RwLock<PrReviewState>>>,
    pr_execution_mode: Arc<std::sync::RwLock<PrExecutionMode>>,
    next_thread_number: Arc<AtomicU64>,
    next_agent_thread_number: Arc<AtomicU64>,
    next_reply_number: Arc<AtomicU64>,
    next_take_number: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(
        state: SharedState,
        bus: Arc<EventBus>,
        emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    ) -> Self {
        Self {
            state,
            bus,
            emitter,
            source: Arc::new(std::sync::RwLock::new(Source::default())),
            file_bytes: Arc::new(HashMap::new()),
            file_versions: Arc::new(HashMap::new()),
            source_path: Arc::new(None),
            history_dir: Arc::new(history::default_history_dir()),
            no_save: Arc::new(AtomicBool::new(false)),
            done_started: Arc::new(AtomicBool::new(false)),
            finalization: Arc::new(Mutex::new(None)),
            completion_scope: Arc::new("session".to_string()),
            shutdown: ShutdownSignal::new(),
            activity: ActivityTracker::new(),
            idle_timeout_secs: Arc::new(AtomicU64::new(Config::default().idle_timeout_secs)),
            verdict_config: Arc::new(None),
            live_frame_url: Arc::new(None),
            demo_scenarios: Arc::new(None),
            offline_demo: Arc::new(AtomicBool::new(false)),
            pr_review: None,
            pr_execution_mode: Arc::new(std::sync::RwLock::new(PrExecutionMode::AgentEvent)),
            next_thread_number: Arc::new(AtomicU64::new(1)),
            next_agent_thread_number: Arc::new(AtomicU64::new(1)),
            next_reply_number: Arc::new(AtomicU64::new(1)),
            next_take_number: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn for_process() -> Self {
        Self::new(
            State::new_shared(),
            Arc::new(EventBus::new(1024)),
            Arc::new(EventEmitter::stdout()),
        )
    }

    pub fn with_source(self, source: Source) -> Self {
        if let Ok(mut current) = self.source.write() {
            *current = source;
        }
        self
    }

    pub fn with_file_bytes(mut self, file_bytes: HashMap<FileId, (Vec<u8>, &'static str)>) -> Self {
        self.file_versions = Arc::new(
            file_bytes
                .iter()
                .map(|(file_id, (bytes, _))| {
                    let digest = Sha256::digest(bytes);
                    (file_id.clone(), format!("{digest:x}"))
                })
                .collect(),
        );
        self.file_bytes = Arc::new(file_bytes);
        self
    }

    pub(super) fn raw_file_version(&self, file_id: &FileId) -> Option<&str> {
        self.file_versions.get(file_id).map(String::as_str)
    }

    pub(super) fn raw_file(&self, file_id: &FileId) -> Option<&(Vec<u8>, &'static str)> {
        let is_image = self
            .source
            .read()
            .ok()
            .and_then(|source| {
                source
                    .files
                    .iter()
                    .find(|file| &file.id == file_id)
                    .map(|file| file.kind == FileKind::Image)
            })
            .unwrap_or(false);
        is_image.then(|| self.file_bytes.get(file_id)).flatten()
    }

    pub(super) fn file_kind(&self, file_id: &FileId) -> Option<FileKind> {
        self.source.read().ok().and_then(|source| {
            if source.files.is_empty() && file_id == &default_file_id() {
                return Some(FileKind::Markdown);
            }
            source
                .files
                .iter()
                .find(|file| &file.id == file_id)
                .map(|file| file.kind)
        })
    }

    /// Single-file convenience used by tests and stdin sessions: replaces the
    /// first file's content (creating a default markdown file if none exist).
    pub fn with_markdown_source(self, markdown_source: impl Into<String>) -> Self {
        let content = markdown_source.into();
        if let Ok(mut source) = self.source.write() {
            if let Some(first) = source.files.first_mut() {
                first.content = content;
            } else {
                source.files.push(File {
                    id: default_file_id(),
                    path: "<stdin>".to_string(),
                    kind: FileKind::Markdown,
                    content,
                });
            }
        }
        self
    }

    pub(super) fn current_source(&self) -> std::result::Result<Source, String> {
        self.source
            .read()
            .map(|source| source.clone())
            .map_err(|_| "source lock poisoned".to_string())
    }

    pub(super) fn primary_file_id(&self) -> FileId {
        self.source
            .read()
            .ok()
            .and_then(|source| source.files.first().map(|file| file.id.clone()))
            .unwrap_or_else(default_file_id)
    }

    pub(super) fn file_ids(&self) -> Vec<FileId> {
        self.source
            .read()
            .map(|source| source.files.iter().map(|file| file.id.clone()).collect())
            .unwrap_or_default()
    }

    pub(super) fn file(&self, file_id: &FileId) -> Option<File> {
        self.source.read().ok().and_then(|source| {
            source
                .files
                .iter()
                .find(|file| &file.id == file_id)
                .cloned()
        })
    }

    pub(super) fn files_count(&self) -> usize {
        self.source
            .read()
            .map(|source| source.files.len())
            .unwrap_or(0)
    }

    pub(super) fn snapshot_with_files(
        &self,
    ) -> std::result::Result<crate::state::StateSnapshot, String> {
        let mut snapshot = self
            .state
            .read()
            .map_err(|_| "state lock poisoned while reading state".to_string())?
            .snapshot();
        snapshot.files = self
            .source
            .read()
            .map_err(|_| "source lock poisoned while reading state".to_string())?
            .files
            .iter()
            .map(crate::state::FileMeta::from)
            .collect();
        snapshot.verdict_config = self.verdict_config.as_ref().clone();
        snapshot.pr_session = self
            .pr_review
            .as_ref()
            .map(|state| {
                state
                    .read()
                    .map(|state| state.snapshot())
                    .map_err(|_| "PR state lock poisoned while reading state".to_string())
            })
            .transpose()?;
        if let Some(pr_session) = snapshot.pr_session.as_mut() {
            pr_session.demo = self.pr_execution_mode() == PrExecutionMode::DemoLocal;
        }
        snapshot.demo_scenarios = self.demo_scenarios.as_ref().clone();
        Ok(snapshot)
    }

    pub fn with_source_path(mut self, source_path: impl Into<PathBuf>) -> Self {
        self.source_path = Arc::new(Some(source_path.into()));
        self
    }

    pub fn with_history_dir(mut self, history_dir: impl Into<PathBuf>) -> Self {
        self.history_dir = Arc::new(history_dir.into());
        self
    }

    pub fn with_no_save(self, no_save: bool) -> Self {
        self.no_save.store(no_save, Ordering::Relaxed);

        self
    }

    pub fn with_verdict_config(mut self, verdict_config: Option<VerdictConfig>) -> Self {
        self.verdict_config = Arc::new(verdict_config);

        self
    }

    pub fn with_live_frame_url(mut self, live_frame_url: impl Into<String>) -> Self {
        self.live_frame_url = Arc::new(Some(live_frame_url.into()));
        self
    }

    pub fn with_demo_scenarios(mut self, scenarios: Vec<DemoScenarioLink>) -> Self {
        self.demo_scenarios = Arc::new(Some(scenarios));
        self
    }

    pub fn with_offline_demo(self) -> Self {
        self.offline_demo.store(true, Ordering::Relaxed);
        self
    }

    pub(crate) fn with_shared_demo_lifecycle(mut self, owner: &AppState) -> Self {
        self.emitter = owner.emitter.clone();
        self.done_started = owner.done_started.clone();
        self.finalization = owner.finalization.clone();
        self.shutdown = owner.shutdown.clone();
        self
    }

    pub(crate) fn with_completion_scope(mut self, scope: impl Into<String>) -> Self {
        self.completion_scope = Arc::new(scope.into());
        self
    }

    pub fn with_pr_session(mut self, identity: GithubPrUrl, secret: String) -> Self {
        self.pr_review = Some(Arc::new(std::sync::RwLock::new(PrReviewState::new(
            identity, secret,
        ))));
        self
    }

    pub fn is_pr_session(&self) -> bool {
        self.pr_review.is_some()
    }

    pub fn with_direct_pr_publication(self) -> Self {
        if let Ok(mut mode) = self.pr_execution_mode.write() {
            *mode = PrExecutionMode::DirectGh;
        }
        self
    }

    pub(crate) fn with_demo_pr_execution(self) -> Self {
        if let Ok(mut mode) = self.pr_execution_mode.write() {
            *mode = PrExecutionMode::DemoLocal;
        }
        self
    }

    pub(super) fn pr_execution_mode(&self) -> PrExecutionMode {
        self.pr_execution_mode
            .read()
            .map(|mode| *mode)
            .unwrap_or(PrExecutionMode::AgentEvent)
    }

    pub(crate) fn set_pr_base_url(&self, base_url: String) {
        if let Some(pr_review) = &self.pr_review
            && let Ok(mut state) = pr_review.write()
        {
            state.api_base_url = Some(base_url);
        }
    }

    pub(super) fn pr_mutations_locked(&self) -> bool {
        self.pr_review
            .as_ref()
            .and_then(|state| state.read().ok().map(|state| state.phase))
            .is_some_and(|phase| matches!(phase, PrPhase::Publishing | PrPhase::Published))
    }

    pub(super) fn live_frame_url(&self) -> Option<&str> {
        self.live_frame_url.as_deref()
    }

    pub(super) fn is_live(&self) -> bool {
        self.live_frame_url.is_some()
    }

    pub(super) fn is_offline_demo(&self) -> bool {
        self.offline_demo.load(Ordering::Relaxed)
    }

    pub(crate) fn signal_shutdown(&self) {
        self.shutdown.signal();
    }

    pub fn with_idle_timeout_secs(self, idle_timeout_secs: u64) -> Self {
        self.idle_timeout_secs
            .store(idle_timeout_secs, Ordering::Relaxed);

        self
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn last_heartbeat_at(&self) -> std::result::Result<Instant, String> {
        self.activity.last_heartbeat_at()
    }

    pub(super) fn record_heartbeat(&self) -> std::result::Result<Instant, String> {
        self.activity.record_heartbeat()
    }

    pub(super) fn record_mutation(&self) {
        if let Err(error) = self.activity.record_mutation() {
            tracing::warn!(error, "failed to update last mutation timestamp");
        }
    }

    pub(super) fn idle_timeout_secs(&self) -> u64 {
        self.idle_timeout_secs.load(Ordering::Relaxed)
    }

    pub(super) fn no_save(&self) -> bool {
        self.no_save.load(Ordering::Relaxed)
    }

    /// Marks the point of no return in `POST /api/done`: called immediately
    /// before the transcript read lock is taken.
    ///
    /// `shutdown` cannot answer "would a mutation still reach the transcript?"
    /// because Done builds and emits the transcript *before* signalling
    /// shutdown. Background writers that must not diverge from the emitted
    /// transcript check `done_started()` instead. `SeqCst` gives a single total
    /// order with the state lock hand-off: a writer that observes `false` while
    /// holding the state write lock necessarily released that lock before Done
    /// could acquire the read lock, so its mutation is in the transcript.
    pub(super) fn begin_done(&self) {
        self.done_started.store(true, Ordering::SeqCst);
    }

    /// Claims terminal transcript finalization for one server-owned operation.
    /// Demo scenarios share this arbiter, so only the scenario that first
    /// reaches Done can emit while the other listeners remain up for a brief
    /// terminal UI grace period. A failed local emission may be retried by the
    /// same owner, but concurrent or completed duplicates are rejected.
    pub(super) fn claim_done(&self, operation: &str) -> bool {
        let owner = format!("{}:{operation}", self.completion_scope);
        let Ok(mut finalization) = self.finalization.lock() else {
            return false;
        };
        match finalization.as_mut() {
            None => {
                *finalization = Some(FinalizationClaim {
                    owner,
                    status: FinalizationStatus::InProgress,
                });
            }
            Some(claim) if claim.owner == owner && claim.status == FinalizationStatus::Failed => {
                claim.status = FinalizationStatus::InProgress;
            }
            Some(_) => return false,
        }
        self.begin_done();
        true
    }

    pub(super) fn fail_done(&self, operation: &str) {
        self.set_finalization_status(operation, FinalizationStatus::Failed);
        self.done_started.store(false, Ordering::SeqCst);
    }

    pub(super) fn complete_done(&self, operation: &str) {
        self.set_finalization_status(operation, FinalizationStatus::Complete);
    }

    fn set_finalization_status(&self, operation: &str, status: FinalizationStatus) {
        let owner = format!("{}:{operation}", self.completion_scope);
        if let Ok(mut finalization) = self.finalization.lock()
            && let Some(claim) = finalization.as_mut()
            && claim.owner == owner
            && claim.status == FinalizationStatus::InProgress
        {
            claim.status = status;
        }
    }

    pub(super) fn done_started(&self) -> bool {
        self.done_started.load(Ordering::SeqCst)
    }

    pub(super) fn next_user_thread_id(&self) -> ThreadId {
        let number = self.next_thread_number.fetch_add(1, Ordering::Relaxed);

        ThreadId(format!("u-{number}"))
    }

    pub(super) fn next_agent_thread_id(&self) -> ThreadId {
        let number = self
            .next_agent_thread_number
            .fetch_add(1, Ordering::Relaxed);

        ThreadId(format!("a-{number}"))
    }

    pub(super) fn next_reply_id(&self) -> String {
        let number = self.next_reply_number.fetch_add(1, Ordering::Relaxed);

        format!("r-{number}")
    }

    pub(super) fn next_take_id(&self) -> String {
        let number = self.next_take_number.fetch_add(1, Ordering::Relaxed);

        format!("t-{number}")
    }
}

#[derive(Clone, Debug)]
pub(super) struct ActivityTracker {
    inner: Arc<Mutex<ActivityState>>,
}

#[derive(Debug)]
pub(super) struct ActivityState {
    last_heartbeat_at: Instant,
    last_mutation_at: Instant,
    last_idle_emit_at: Option<Instant>,
}

impl ActivityTracker {
    pub(super) fn new() -> Self {
        let now = Instant::now();

        Self {
            inner: Arc::new(Mutex::new(ActivityState {
                last_heartbeat_at: now,
                last_mutation_at: now,
                last_idle_emit_at: None,
            })),
        }
    }

    pub(super) fn last_heartbeat_at(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|state| state.last_heartbeat_at)
            .map_err(|_| "activity lock poisoned".to_string())
    }

    pub(super) fn record_heartbeat(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|mut state| {
                let now = Instant::now();
                state.last_heartbeat_at = now;
                now
            })
            .map_err(|_| "activity lock poisoned".to_string())
    }

    pub(super) fn record_mutation(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|mut state| {
                let now = Instant::now();
                state.last_mutation_at = now;
                now
            })
            .map_err(|_| "activity lock poisoned".to_string())
    }

    pub(super) fn record_idle_prompt_if_due(
        &self,
        now: Instant,
        idle_timeout: Duration,
    ) -> std::result::Result<Option<Duration>, String> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "activity lock poisoned".to_string())?;
        let last_activity_at = state.last_heartbeat_at.max(state.last_mutation_at);
        let idle_for = now.saturating_duration_since(last_activity_at);

        if idle_for < idle_timeout {
            return Ok(None);
        }

        if let Some(last_idle_emit_at) = state.last_idle_emit_at {
            let already_emitted_for_current_window = last_idle_emit_at >= last_activity_at
                && now.saturating_duration_since(last_idle_emit_at) < idle_timeout;
            if already_emitted_for_current_window {
                return Ok(None);
            }
        }

        state.last_idle_emit_at = Some(now);

        Ok(Some(idle_for))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::for_process()
    }
}

#[derive(Clone, Debug)]
pub(super) struct ShutdownSignal {
    tx: watch::Sender<bool>,
}

impl ShutdownSignal {
    pub(super) fn new() -> Self {
        let (tx, _) = watch::channel(false);

        Self { tx }
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    pub(super) fn signal(&self) {
        self.tx.send_replace(true);
    }

    pub(super) fn is_signaled(&self) -> bool {
        *self.tx.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_execution_mode_is_exclusive_and_demo_is_offline() {
        let direct = AppState::for_process().with_direct_pr_publication();
        let earlier_clone = direct.clone();
        assert_eq!(direct.pr_execution_mode(), PrExecutionMode::DirectGh);

        let demo = direct.with_demo_pr_execution().with_offline_demo();
        assert_eq!(demo.pr_execution_mode(), PrExecutionMode::DemoLocal);
        assert_eq!(
            earlier_clone.pr_execution_mode(),
            PrExecutionMode::DemoLocal
        );
        assert!(demo.is_offline_demo());
    }

    #[test]
    fn demo_states_share_shutdown_and_done_latches_only_when_requested() {
        let owner = AppState::for_process().with_completion_scope("tour");
        let peer = AppState::for_process()
            .with_shared_demo_lifecycle(&owner)
            .with_completion_scope("local-app");
        assert!(peer.claim_done("done"));
        assert!(!peer.claim_done("done"), "concurrent duplicate is rejected");
        assert!(!owner.claim_done("done"));
        peer.fail_done("done");
        assert!(peer.claim_done("done"), "failed owner may retry");
        peer.complete_done("done");
        assert!(!peer.claim_done("done"), "completed duplicate is rejected");
        peer.signal_shutdown();
        assert!(owner.done_started());
        assert!(owner.shutdown.is_signaled());
    }
}
