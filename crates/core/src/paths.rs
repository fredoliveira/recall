// SPDX-License-Identifier: MIT

//! MTGA `Player.log` discovery.
//!
//! Port `POSSIBLE_ROOTS` × {`Player.log`, `Player-prev.log`} verbatim from
//! `mtga_follower.py:54-128`: OSX `~/Library/Logs`, Steam Proton compatdata `2141910`,
//! Lutris, Wine (`$WINEPREFIX`), Windows `C:/`+`D:/` `users/<user>/AppData/LocalLow`.

use std::path::PathBuf;

const CURRENT_LOG: &str = "Player.log";
const PREVIOUS_LOG: &str = "Player-prev.log";

/// `getpass.getuser()` equivalent: first non-empty of `LOGNAME`, `USER`, `LNAME`,
/// `USERNAME` (the env vars CPython's `getpass.getuser` consults, in order).
fn current_username() -> String {
    for var in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            return value;
        }
    }
    // CPython falls back to the pwd database; we have no portable equivalent, so use an
    // empty placeholder. The resulting Windows-style paths won't exist on this host anyway.
    String::new()
}

/// `users/<user>/AppData/LocalLow`.
fn windows_log_root() -> PathBuf {
    PathBuf::from("users")
        .join(current_username())
        .join("AppData")
        .join("LocalLow")
}

/// `steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow`.
fn steam_log_root() -> PathBuf {
    [
        "steamapps",
        "compatdata",
        "2141910",
        "pfx",
        "drive_c",
        "users",
        "steamuser",
        "AppData",
        "LocalLow",
    ]
    .iter()
    .collect()
}

/// `Wizards Of The Coast/MTGA`.
fn log_intermediate() -> PathBuf {
    PathBuf::from("Wizards Of The Coast").join("MTGA")
}

/// `POSSIBLE_ROOTS` (`mtga_follower.py:78-115`), in priority order.
fn possible_roots() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let wineprefix = std::env::var_os("WINEPREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".wine"));

    vec![
        // OSX
        home.join("Library").join("Logs"),
        // Steam
        home.join(".steam").join("steam").join(steam_log_root()),
        home.join(".local")
            .join("share")
            .join("Steam")
            .join(steam_log_root()),
        // Windows
        PathBuf::from("C:/").join(windows_log_root()),
        PathBuf::from("D:/").join(windows_log_root()),
        // Lutris
        home.join("Games")
            .join("magic-the-gathering-arena")
            .join("drive_c")
            .join(windows_log_root()),
        // Wine
        wineprefix.join("drive_c").join(windows_log_root()),
    ]
}

fn filepaths_for(log_name: &str) -> Vec<PathBuf> {
    let suffix = log_intermediate().join(log_name);
    possible_roots()
        .into_iter()
        .map(|root| root.join(&suffix))
        .collect()
}

/// Candidate current-log paths (`Player.log`), in priority order.
pub fn possible_current_filepaths() -> Vec<PathBuf> {
    filepaths_for(CURRENT_LOG)
}

/// Candidate previous-log paths (`Player-prev.log`), in priority order.
pub fn possible_previous_filepaths() -> Vec<PathBuf> {
    filepaths_for(PREVIOUS_LOG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_roots_each() {
        assert_eq!(possible_current_filepaths().len(), 7);
        assert_eq!(possible_previous_filepaths().len(), 7);
    }

    #[test]
    fn osx_path_is_first_and_well_formed() {
        let first = &possible_current_filepaths()[0];
        // `Path::ends_with` matches on whole path components, so this is separator-agnostic
        // - a plain `to_string_lossy().contains("a/b")` would fail on Windows, where
        // `PathBuf::join` emits backslashes.
        assert!(
            first.ends_with("Library/Logs/Wizards Of The Coast/MTGA/Player.log"),
            "{}",
            first.display()
        );
    }

    #[test]
    fn previous_uses_prev_filename() {
        for p in possible_previous_filepaths() {
            assert!(p.to_string_lossy().ends_with("Player-prev.log"));
        }
    }
}
