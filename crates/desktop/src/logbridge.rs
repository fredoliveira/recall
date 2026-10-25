//! Bridges the core's `log` output into the webview.
//!
//! The follower has no callback/observer hooks - everything it does is reported through the
//! `log` crate. We install a global `log::Log` sink that (a) keeps a capped ring buffer so a
//! freshly-opened window can backfill recent lines, and (b) emits each line as a `log-line`
//! event to the webview. The logger is installed before the Tauri app exists; [`attach`] wires
//! in the `AppHandle` once `setup` runs.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Max lines retained for window backfill.
const CAPACITY: usize = 2000;

#[derive(Clone, Serialize)]
pub struct LogLine {
    pub ts: String,
    pub level: String,
    pub target: String,
    pub msg: String,
    /// Background-sync chatter (`follower::CHATTER`) - rendered dimmed by the UI.
    pub chatter: bool,
}

static BUFFER: Mutex<VecDeque<LogLine>> = Mutex::new(VecDeque::new());
static APP: OnceLock<AppHandle> = OnceLock::new();

pub struct WebviewLogger;

impl WebviewLogger {
    /// Install as the global logger. Safe to call once at startup.
    pub fn install() {
        if log::set_boxed_logger(Box::new(WebviewLogger)).is_ok() {
            // Debug needed: the 17Lands transport logs upload success ("<endpoint> -> 200") at debug
            // (failures and rate limiting are WARN/ERROR).
            log::set_max_level(LevelFilter::Debug);
        }
    }
}

/// Wire the live `AppHandle` so subsequent records are emitted to the webview.
pub fn attach(app: AppHandle) {
    let _ = APP.set(app);
}

/// Snapshot of the retained lines (for window backfill).
pub fn recent() -> Vec<LogLine> {
    BUFFER
        .lock()
        .map(|b| b.iter().cloned().collect())
        .unwrap_or_default()
}

/// Keep our own crates' chatter; drop noisy debug from third-party deps (ureq, etc.).
fn is_ours(target: &str) -> bool {
    target.starts_with("recall")
}

impl Log for WebviewLogger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        let target = record.target();
        if !is_ours(target) {
            return;
        }

        let line = LogLine {
            ts: chrono::Local::now().format("%H:%M:%S").to_string(),
            level: record.level().to_string(),
            target: target.to_string(),
            msg: record.args().to_string(),
            chatter: target == recall_core::follower::CHATTER,
        };

        if let Ok(mut buf) = BUFFER.lock() {
            if buf.len() >= CAPACITY {
                buf.pop_front();
            }
            buf.push_back(line.clone());
        }

        // Echo to stderr too, so `cargo tauri dev` shows the same feed in the terminal.
        eprintln!("{} {:<5} {}", line.ts, line.level, line.msg);

        if let Some(app) = APP.get() {
            let _ = app.emit("log-line", line);
        }
    }

    fn flush(&self) {}
}
