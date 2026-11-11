//! App state: owns the follower thread and its cancellation, the resolved log path, the
//! cached config and the shared upload status. Managed by Tauri (`app.manage`) so commands
//! and the tray can reach it.
//!
//! Nothing polls: every change to what the tray and window display (follower start, stop or
//! exit, a saved token, an upload outcome) calls [`AppState::notify`], which re-renders the
//! tray line and emits `status-changed` to the webview.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use serde::Serialize;
use tauri::AppHandle;

use recall_core::config::Config;
use recall_core::follower::Follower;
use recall_core::services::seventeenlands::ApiClient;
use recall_core::{paths, services};

use crate::observer::{ObservingSubmitter, UploadStatus};

pub struct AppState {
    /// `RECALL_HOST` dev override; otherwise the host comes from the config at start.
    host_override: Option<String>,
    cancel: Arc<AtomicBool>,
    following: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    log_path: Mutex<Option<PathBuf>>,
    /// Byte read-position published by the follower thread; lets stop→start resume the tail
    /// instead of re-reading the whole log. Paired with `offset_path` so a stale offset is
    /// only reused when the resolved log path is unchanged.
    read_offset: Arc<AtomicU64>,
    /// The log path that `read_offset` was recorded against.
    offset_path: Mutex<Option<PathBuf>>,
    pub upload: Arc<Mutex<UploadStatus>>,
    /// The config as last read from disk. Refreshed on every start (so after a token save)
    /// and when the window gains focus, which picks up a hand-edit of `config.toml`.
    cfg: Mutex<Config>,
    /// Set once in `setup`; before that, `notify` is a no-op.
    app: OnceLock<AppHandle>,
}

/// Status snapshot serialized to the UI and used to render tray text.
#[derive(Serialize)]
pub struct StatusDto {
    pub following: bool,
    pub token_present: bool,
    pub log_path: Option<String>,
    pub host: String,
    pub upload_count: u64,
    pub last_endpoint: Option<String>,
    pub last_time: Option<String>,
    pub upload_failed: u64,
    pub last_failure: Option<String>,
    /// Enabled service names, primary first (e.g. `["seventeenlands"]`).
    pub services: Vec<String>,
    /// Crate version (e.g. "0.1.1"), rendered in the in-window header.
    pub app_version: String,
}

impl AppState {
    pub fn new(host_override: Option<String>) -> Self {
        // Dev/test override (parallels RECALL_HOST): pin the followed log file so the
        // app can be exercised headlessly against a fixture log.
        let log_path = std::env::var("RECALL_LOG").ok().map(PathBuf::from);
        Self {
            host_override,
            cancel: Arc::new(AtomicBool::new(false)),
            following: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
            log_path: Mutex::new(log_path),
            read_offset: Arc::new(AtomicU64::new(0)),
            offset_path: Mutex::new(None),
            upload: Arc::new(Mutex::new(UploadStatus::default())),
            cfg: Mutex::new(Config::load()),
            app: OnceLock::new(),
        }
    }

    /// Wire in the live `AppHandle` so status changes can reach the tray and the webview.
    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// Tell the tray and the webview that the displayed status changed.
    pub fn notify(&self) {
        if let Some(app) = self.app.get() {
            crate::status_changed(app);
        }
    }

    /// The cached config (see the `cfg` field for when it is refreshed).
    pub fn config(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    /// Re-read `config.toml` into the cache.
    pub fn reload_config(&self) -> Config {
        let cfg = Config::load();
        *self.cfg.lock().unwrap() = cfg.clone();
        cfg
    }

    /// The user-chosen path if set, else the first auto-detected `Player.log` that exists.
    pub fn resolve_log_path(&self) -> Option<PathBuf> {
        if let Some(p) = self.log_path.lock().unwrap().clone() {
            return Some(p);
        }
        paths::possible_current_filepaths()
            .into_iter()
            .find(|p| p.exists())
    }

    pub fn set_log_path(&self, p: Option<PathBuf>) {
        *self.log_path.lock().unwrap() = p;
    }

    /// Upload host: `RECALL_HOST` if set, else `services.seventeenlands.host`, else live.
    fn host_for(&self, cfg: &Config) -> String {
        self.host_override
            .clone()
            .unwrap_or_else(|| cfg.seventeenlands_host())
    }

    pub fn is_following(&self) -> bool {
        self.following.load(Ordering::Relaxed)
    }

    /// Spawn the follower on a dedicated thread. No-op if already running.
    pub fn start(&self) -> Result<(), String> {
        if self.is_following() {
            return Ok(());
        }
        let cfg = self.reload_config();
        let token = cfg
            .seventeenlands_token()
            .ok_or("No valid token configured")?;
        let path = self
            .resolve_log_path()
            .ok_or("No Player.log found - set a log path in Settings")?;
        let path_str = path.to_string_lossy().to_string();

        // Resume from the remembered offset only when the resolved path is unchanged since the
        // last run; a different log (e.g. one picked in Settings) starts from the beginning so
        // we never seek into an unrelated file.
        let start_offset = if self.offset_path.lock().unwrap().as_deref() == Some(path.as_path()) {
            self.read_offset.load(Ordering::Relaxed)
        } else {
            0
        };
        self.read_offset.store(start_offset, Ordering::Relaxed);

        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let following = self.following.clone();
        let host = self.host_for(&cfg);
        let upload = self.upload.clone();
        let read_offset = self.read_offset.clone();
        // Both callbacks run on the follower thread; emitting from there is fine.
        let app = self.app.get().cloned();
        let notify = move || {
            if let Some(app) = &app {
                crate::status_changed(app);
            }
        };

        let handle = std::thread::Builder::new()
            .name("recall-follower".into())
            .spawn(move || {
                following.store(true, Ordering::Relaxed);
                notify();
                let wire =
                    ObservingSubmitter::new(ApiClient::new(host), upload, Box::new(notify.clone()));
                let mut follower = Follower::with_sink(services::from_config(&cfg, token, wire));
                follower.parse_log_cancellable_from(
                    &path_str,
                    true,
                    &cancel,
                    start_offset,
                    Some(&read_offset),
                );
                following.store(false, Ordering::Relaxed);
                notify();
            })
            .map_err(|e| e.to_string())?;

        *self.thread.lock().unwrap() = Some(handle);
        *self.log_path.lock().unwrap() = Some(path.clone());
        *self.offset_path.lock().unwrap() = Some(path);
        Ok(())
    }

    /// Signal cancellation and join the follower thread (returns within ~one 500ms tick).
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        self.following.store(false, Ordering::Relaxed);
        self.notify();
    }

    pub fn restart(&self) -> Result<(), String> {
        self.stop();
        self.start()
    }

    pub fn status(&self) -> StatusDto {
        let u = self.upload.lock().unwrap().clone();
        let cfg = self.config();
        StatusDto {
            following: self.is_following(),
            token_present: cfg.seventeenlands_token().is_some(),
            log_path: self
                .resolve_log_path()
                .map(|p| p.to_string_lossy().to_string()),
            host: self.host_for(&cfg),
            upload_count: u.count,
            last_endpoint: u.last_endpoint,
            last_time: u.last_time,
            upload_failed: u.failed,
            last_failure: u.last_failure,
            services: cfg.service_names(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}
