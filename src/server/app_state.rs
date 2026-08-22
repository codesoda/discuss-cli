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
use crate::sse::EventBus;
use crate::state::{File, FileId, FileKind, SharedState, Source, State, ThreadId, default_file_id};
use crate::verdict::VerdictConfig;

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
    pub(super) shutdown: ShutdownSignal,
    pub(super) activity: ActivityTracker,
    idle_timeout_secs: Arc<AtomicU64>,
    pub(super) verdict_config: Arc<Option<VerdictConfig>>,
    next_thread_number: Arc<AtomicU64>,
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
            shutdown: ShutdownSignal::new(),
            activity: ActivityTracker::new(),
            idle_timeout_secs: Arc::new(AtomicU64::new(Config::default().idle_timeout_secs)),
            verdict_config: Arc::new(None),
            next_thread_number: Arc::new(AtomicU64::new(1)),
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

    pub(super) fn next_user_thread_id(&self) -> ThreadId {
        let number = self.next_thread_number.fetch_add(1, Ordering::Relaxed);

        ThreadId(format!("u-{number}"))
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
