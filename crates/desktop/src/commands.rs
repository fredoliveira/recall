//! `#[tauri::command]` bridge between the webview and `AppState` / core config.

use tauri::State;

use recall_core::config::{self, Config};

use crate::logbridge::{self, LogLine};
use crate::state::{AppState, StatusDto};

#[tauri::command]
pub fn token_present() -> bool {
    Config::load().seventeenlands_token().is_some()
}

/// Validate (UUID v4) and persist the token to the same file the CLI reads, then (re)start.
#[tauri::command]
pub fn save_token(token: String, state: State<AppState>) -> Result<(), String> {
    let valid = config::validate_uuid_v4(token.trim())
        .ok_or("That doesn't look like a valid 17Lands token (expected a UUID).")?;
    // Strict: a config that exists but won't parse must not be overwritten with defaults
    // (that would silently drop the user's archive/host settings).
    let mut cfg = Config::load_strict()
        .map_err(|e| format!("Could not read the existing config; fix or remove it first: {e}"))?;
    cfg.set_seventeenlands_token(&valid);
    cfg.save()
        .map_err(|e| format!("Could not save config: {e}"))?;
    let result = state.restart();
    // Even when the follower could not start, the token is now present.
    state.notify();
    result
}

#[tauri::command]
pub fn get_status(state: State<AppState>) -> StatusDto {
    state.status()
}

#[tauri::command]
pub fn recent_logs() -> Vec<LogLine> {
    logbridge::recent()
}

/// Point the follower at a specific log file (overrides auto-detection), then restart.
#[tauri::command]
pub fn set_log_path(path: String, state: State<AppState>) -> Result<(), String> {
    let pb = std::path::PathBuf::from(&path);
    if !pb.exists() {
        return Err(format!("No file at {path}"));
    }
    state.set_log_path(Some(pb));
    let result = state.restart();
    state.notify();
    result
}
