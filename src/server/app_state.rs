//! Shared server state: the axum `AppState` handle plus the activity and
//! shutdown primitives it owns.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::Config;
use crate::events::{Event, EventEmitter, EventKind};
use crate::history;
use crate::pr::{GithubPrUrl, PrPhase, PrReviewState};
use crate::prefs;
use crate::sse::{BroadcastEvent, EventBus};
use crate::state::{
    DemoScenarioLink, File, FileId, FileKind, SharedState, Source, State, ThreadId, default_file_id,
};
use crate::verdict::VerdictConfig;

/// How long after the last block edit the file is written. A burst of edits
/// becomes one disk write per file.
const SOURCE_WRITE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Write-behind bookkeeping for persisting user block edits to their files.
#[derive(Debug, Default)]
struct SourceSyncState {
    /// Per file: the content we believe is on disk — what we loaded at
    /// launch, last wrote, or an agent last pushed. The external-change guard
    /// compares the file against this before every write.
    last_synced: HashMap<FileId, Arc<str>>,
    /// Per file: bumped per scheduled write; a sleeping writer only runs if
    /// it is still the newest, which debounces rapid edits into one write.
    write_generations: HashMap<FileId, u64>,
}

/// The on-disk path a file's edits persist to: markdown files loaded from a
/// real (absolute) path. Stdin docs (`<stdin>`), diff panes, images and HTML
/// prototypes have no writable markdown source.
pub(super) fn persistable_file_path(file: &File) -> Option<PathBuf> {
    if file.kind != FileKind::Markdown {
        return None;
    }
    let path = Path::new(&file.path);
    path.is_absolute().then(|| path.to_path_buf())
}

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
    source_sync: Arc<Mutex<SourceSyncState>>,
    source_write_lock: Arc<tokio::sync::Mutex<()>>,
    pub(super) file_bytes: Arc<HashMap<FileId, (Vec<u8>, &'static str)>>,
    file_versions: Arc<HashMap<FileId, String>>,
    pub(super) source_path: Arc<Option<PathBuf>>,
    pub(super) history_dir: Arc<PathBuf>,
    pub(super) prefs_path: Arc<PathBuf>,
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
            source_sync: Arc::new(Mutex::new(SourceSyncState::default())),
            source_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            file_bytes: Arc::new(HashMap::new()),
            file_versions: Arc::new(HashMap::new()),
            source_path: Arc::new(None),
            history_dir: Arc::new(history::default_history_dir()),
            prefs_path: Arc::new(prefs::default_prefs_path()),
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
        if let Ok(mut sync) = self.source_sync.lock() {
            sync.last_synced.clear();
            for file in &source.files {
                if persistable_file_path(file).is_some() {
                    sync.last_synced
                        .insert(file.id.clone(), Arc::from(file.content.as_str()));
                }
            }
        }
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

    /// Whether user edits to this file reach its path on disk. Diff panes,
    /// stdin docs, and --no-save runs live only in this process.
    pub(super) fn file_persisted(&self, file: &File) -> bool {
        !self.no_save() && persistable_file_path(file).is_some()
    }

    /// An agent pushed this content through `POST /api/source`: it owns the
    /// file, so treat its push as what is now on disk. Without this a user
    /// edit after any agent push would trip the external-change guard.
    pub(super) fn note_agent_synced(&self, file_id: &FileId, content: &str) {
        if let Ok(mut sync) = self.source_sync.lock()
            && sync.last_synced.contains_key(file_id)
        {
            sync.last_synced.insert(file_id.clone(), Arc::from(content));
        }
    }

    /// Debounced write-behind: bump the file's generation and let only the
    /// newest scheduled writer actually persist.
    pub(super) fn schedule_source_write(&self, file: &File) {
        if self.no_save() {
            return;
        }
        let Some(path) = persistable_file_path(file) else {
            return;
        };
        let file_id = file.id.clone();
        let generation = {
            let Ok(mut sync) = self.source_sync.lock() else {
                return;
            };
            let slot = sync.write_generations.entry(file_id.clone()).or_insert(0);
            *slot += 1;
            *slot
        };
        let app_state = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SOURCE_WRITE_DEBOUNCE).await;
            let still_newest = app_state
                .source_sync
                .lock()
                .map(|sync| sync.write_generations.get(&file_id) == Some(&generation))
                .unwrap_or(false);
            if still_newest {
                app_state.persist_source(file_id, path).await;
            }
        });
    }

    pub(super) async fn flush_source_writes(&self) {
        if self.no_save() {
            return;
        }
        // Called after requests drain. Removing generations also invalidates
        // sleeping debounce tasks; already-running writers share the I/O lock.
        let pending: Vec<_> = match self.source_sync.lock() {
            Ok(mut sync) => sync.write_generations.drain().map(|(id, _)| id).collect(),
            Err(_) => {
                tracing::warn!("could not flush source edits: source sync lock poisoned");
                return;
            }
        };
        for id in pending {
            if let Some(path) = self.file(&id).and_then(|file| persistable_file_path(&file)) {
                self.clone().persist_source(id, path).await;
            }
        }
    }

    async fn persist_source(self, file_id: FileId, path: PathBuf) {
        // Debouncing only cancels sleeping writers. Serialize writers already
        // doing I/O too, including their snapshots and last_synced updates.
        let _write_guard = self.source_write_lock.lock().await;
        // The newest content wins: read it at write time, not schedule time.
        let Some(file) = self.file(&file_id) else {
            return;
        };
        let current: Arc<str> = Arc::from(file.content.as_str());
        let last_synced = match self.source_sync.lock() {
            Ok(sync) => sync.last_synced.get(&file_id).cloned(),
            Err(_) => {
                self.emit_save_failed("io_error", "synced source lock poisoned".into(), &path);
                return;
            }
        };

        // Detect external changes before staging and again before replacement.
        // A missing file is ours to recreate; anything else unreadable is an
        // error worth surfacing.
        let disk_before = match tokio::fs::read_to_string(&path).await {
            Ok(on_disk) => {
                if last_synced.as_deref() != Some(on_disk.as_str()) {
                    self.emit_save_failed(
                        "external_change",
                        format!(
                            "{} changed outside discuss; edits stay in memory until the review is restarted",
                            path.display()
                        ),
                        &path,
                    );
                    return;
                }
                Some(on_disk)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                self.emit_save_failed(
                    "io_error",
                    format!("failed to read {}: {error}", path.display()),
                    &path,
                );
                return;
            }
        };

        // Atomic replace: temp file in the same directory, then rename, so a
        // crash mid-write can never truncate the document.
        // Create exclusively with restrictive permissions; a predictable path
        // could collide with another writer (or follow an existing symlink).
        let temp = match tempfile::Builder::new()
            .prefix(".discuss-tmp-")
            .tempfile_in(path.parent().unwrap_or_else(|| Path::new(".")))
        {
            Ok(temp) => temp.into_temp_path(),
            Err(error) => {
                self.emit_save_failed(
                    "io_error",
                    format!("failed to create temporary source: {error}"),
                    &path,
                );
                return;
            }
        };
        let temp_path: &Path = temp.as_ref();
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => {
                if let Err(error) =
                    tokio::fs::set_permissions(temp_path, metadata.permissions()).await
                {
                    self.emit_save_failed(
                        "io_error",
                        format!("failed to preserve source permissions: {error}"),
                        &path,
                    );
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                self.emit_save_failed(
                    "io_error",
                    format!("failed to read source permissions: {error}"),
                    &path,
                );
                return;
            }
        }
        if let Err(error) = tokio::fs::write(&temp_path, current.as_bytes()).await {
            self.emit_save_failed(
                "io_error",
                format!("failed to write {}: {error}", temp_path.display()),
                &path,
            );
            let _ = tokio::fs::remove_file(&temp_path).await;
            return;
        }
        let destination = path.clone();
        let replaced = tokio::task::spawn_blocking(move || {
            replace_source_if_unchanged(&temp, &destination, disk_before.as_deref())
        })
        .await;
        match replaced {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                self.emit_save_failed(
                    "external_change",
                    format!(
                        "{} changed while saving; edits remain in memory",
                        path.display()
                    ),
                    &path,
                );
                return;
            }
            error => {
                self.emit_save_failed(
                    "io_error",
                    format!("failed to replace {}: {error:?}", path.display()),
                    &path,
                );
                return;
            }
        }
        if let Ok(mut sync) = self.source_sync.lock() {
            sync.last_synced.insert(file_id, current);
        }
    }

    fn emit_save_failed(&self, reason: &str, message: String, path: &Path) {
        tracing::warn!(reason, %message, "source save failed");
        let payload = serde_json::json!({
            "reason": reason,
            "message": message,
            "path": path.to_string_lossy(),
        });
        self.bus.publish(BroadcastEvent {
            kind: EventKind::SourceSaveFailed.to_string(),
            payload: payload.clone(),
        });
        if let Err(error) = self.emitter.emit(&Event {
            kind: EventKind::SourceSaveFailed,
            at: Utc::now(),
            payload,
        }) {
            tracing::warn!(error = %error, "failed to emit source.save_failed event");
        }
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

    pub fn with_prefs_path(mut self, prefs_path: impl Into<PathBuf>) -> Self {
        self.prefs_path = Arc::new(prefs_path.into());
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

// Recheck after staging, without an async scheduling gap before rename.
// This is best-effort: portable filesystems offer no content compare-and-swap
// against uncooperative external writers in the final read/rename interval.
fn replace_source_if_unchanged(
    temp: &Path,
    path: &Path,
    expected: Option<&str>,
) -> io::Result<bool> {
    let actual = match std::fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if actual.as_deref() != expected {
        return Ok(false);
    }
    std::fs::rename(temp, path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_source_check_rejects_changes_after_staging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.md");
        let temp = dir.path().join("staged.md");
        std::fs::write(&path, "original").unwrap();
        let expected = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&temp, "user edit").unwrap();
        std::fs::write(&path, "external edit").unwrap();
        assert!(!replace_source_if_unchanged(&temp, &path, Some(&expected)).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external edit");
        std::fs::remove_file(&path).unwrap();
        assert!(!replace_source_if_unchanged(&temp, &path, Some(&expected)).unwrap());
        assert!(replace_source_if_unchanged(&temp, &path, None).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user edit");
    }

    #[tokio::test]
    async fn queued_source_writers_snapshot_after_acquiring_the_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.md");
        std::fs::write(&path, "original").unwrap();
        let app = AppState::for_process().with_source(Source {
            files: vec![File {
                id: default_file_id(),
                path: path.to_string_lossy().into_owned(),
                kind: FileKind::Markdown,
                content: "original".into(),
            }],
        });
        let mut events = app.bus.subscribe();
        let guard = app.source_write_lock.lock().await;
        let mut writers = Vec::new();
        for _ in 0..8 {
            writers.push(tokio::spawn(
                app.clone().persist_source(default_file_id(), path.clone()),
            ));
        }
        tokio::task::yield_now().await;
        assert!(writers.iter().all(|writer| !writer.is_finished()));
        app.source.write().unwrap().files[0].content = "latest".into();
        drop(guard);
        for writer in writers {
            writer.await.unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "latest");
        assert_eq!(
            app.source_sync.lock().unwrap().last_synced[&default_file_id()].as_ref(),
            "latest"
        );
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

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
