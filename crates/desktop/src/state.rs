//! App state: owns the follower thread and its cancellation, the resolved log path, the
//! cached config and the shared upload status. Managed by Tauri (`app.manage`) so commands
//! and the tray can reach it.
//!
//! Nothing polls: every change to what the tray and window display (follower start, stop or
//! exit, a saved token, an upload outcome) calls [`AppState::notify`], which re-renders the
//! tray line and emits `status-changed` to the webview.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use serde::Serialize;
use tauri::AppHandle;

use recall_core::checkpoint::{self, Checkpoints};
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

        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let following = self.following.clone();
        let host = self.host_for(&cfg);
        let upload = self.upload.clone();
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
                // Loaded per start: a stop→start (or an app relaunch) replays the log up to
                // the checkpoint with the services muted, then carries on live.
                let mut checkpoints = Checkpoints::load(checkpoint::scope_for(&host, &token));
                // The client shares the cancel flag, so a stop also ends a retry backoff.
                let client = ApiClient::new(host).with_cancel(cancel.clone());
                let wire = ObservingSubmitter::new(client, upload, Box::new(notify.clone()));
                let mut follower = Follower::with_sink(services::from_config(&cfg, token, wire));
                follower.parse_log_resumable(&path_str, true, &cancel, Some(&mut checkpoints));
                following.store(false, Ordering::Relaxed);
                notify();
            })
            .map_err(|e| e.to_string())?;

        *self.thread.lock().unwrap() = Some(handle);
        *self.log_path.lock().unwrap() = Some(path);
        Ok(())
    }

    /// Signal cancellation and join the follower thread. Returns within ~one 500ms tick,
    /// also while an upload is backing off; only a request already on the wire can hold it
    /// longer (up to the HTTP timeouts).
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
