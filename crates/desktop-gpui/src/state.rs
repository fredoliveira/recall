//! App state: owns the follower thread and its cancellation, the resolved log path, the
//! cached config and the shared upload status.
//!
//! Nothing polls: every change to what the windows display (follower start, stop or exit, a
//! saved token, an upload outcome) calls [`AppState::notify`], which sends
//! [`Signal::Status`] to the main thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use smol::channel::Sender;

use recall_core::checkpoint::{self, Checkpoints};
use recall_core::config::{self, Config};
use recall_core::follower::Follower;
use recall_core::services::seventeenlands::ApiClient;
use recall_core::{paths, services};

use crate::feed::Signal;
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
    signals: Sender<Signal>,
}

/// A snapshot of everything the windows show about the follower.
#[derive(Clone, Default)]
pub struct Status {
    pub following: bool,
    pub token_present: bool,
    /// Last four characters of the saved token, so the UI can show which one is in use
    /// without ever holding the whole secret.
    pub token_hint: Option<String>,
    pub log_path: Option<String>,
    pub host: String,
    pub upload_count: u64,
    pub last_endpoint: Option<String>,
    pub last_time: Option<String>,
    pub upload_failed: u64,
    pub last_failure: Option<String>,
    /// Enabled service names, primary first (e.g. `["seventeenlands"]`).
    pub services: Vec<String>,
}

impl AppState {
    pub fn new(host_override: Option<String>, signals: Sender<Signal>) -> Self {
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
            signals,
        }
    }

    /// Tell the windows that the displayed status changed. Never blocks.
    pub fn notify(&self) {
        let _ = self.signals.try_send(Signal::Status);
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
        paths::possible_current_filepaths().into_iter().find(|p| p.exists())
    }

    pub fn set_log_path(&self, p: Option<PathBuf>) {
        *self.log_path.lock().unwrap() = p;
    }

    /// Upload host: `RECALL_HOST` if set, else `services.seventeenlands.host`, else live.
    fn host_for(&self, cfg: &Config) -> String {
        self.host_override.clone().unwrap_or_else(|| cfg.seventeenlands_host())
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
        let token = cfg.seventeenlands_token().ok_or("No valid token configured")?;
        let path = self.resolve_log_path().ok_or("No Player.log found - set a log path in Settings")?;
        let path_str = path.to_string_lossy().to_string();

        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let following = self.following.clone();
        let host = self.host_for(&cfg);
        let upload = self.upload.clone();
        // Runs on the follower thread; sending from there never blocks.
        let signals = self.signals.clone();
        let notify = move || {
            let _ = signals.try_send(Signal::Status);
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

    /// Validate (UUID v4) and save the token to the file the CLI reads, then (re)start.
    pub fn save_token(&self, token: &str) -> Result<(), String> {
        let valid = config::validate_uuid_v4(token.trim())
            .ok_or("That doesn't look like a 17Lands token (it should be a UUID).")?;
        // Strict: a config that exists but won't parse must not be overwritten with defaults
        // (that would silently drop the user's archive and host settings).
        let mut cfg = Config::load_strict()
            .map_err(|e| format!("Couldn't read the existing config; fix or remove it first: {e}"))?;
        cfg.set_seventeenlands_token(&valid);
        cfg.save().map_err(|e| format!("Couldn't save the config: {e}"))?;
        let result = self.restart();
        // Even when the follower could not start, the token is now present.
        self.notify();
        result
    }

    /// Follow this log file instead of the auto-detected one, then restart.
    pub fn follow(&self, path: &str) -> Result<(), String> {
        let path = PathBuf::from(path.trim());
        if !path.is_file() {
            return Err(format!("There's no file at {}", path.display()));
        }
        self.set_log_path(Some(path));
        let result = self.restart();
        self.notify();
        result
    }

    pub fn status(&self) -> Status {
        let u = self.upload.lock().unwrap().clone();
        let cfg = self.config();
        let token = cfg.seventeenlands_token();
        Status {
            following: self.is_following(),
            token_present: token.is_some(),
            token_hint: token.map(|t| t[t.len().saturating_sub(4)..].to_string()),
            log_path: self.resolve_log_path().map(|p| p.to_string_lossy().to_string()),
            host: self.host_for(&cfg),
            upload_count: u.count,
            last_endpoint: u.last_endpoint,
            last_time: u.last_time,
            upload_failed: u.failed,
            last_failure: u.last_failure,
            services: cfg.service_names(),
        }
    }
}
