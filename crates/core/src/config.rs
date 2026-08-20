// SPDX-License-Identifier: MIT

//! Per-service configuration at `<config_dir>/recall/config.toml`.
//!
//! ```toml
//! [services.seventeenlands]
//! token = "00000000-0000-4000-8000-000000000000"
//! # host = "https://api.17lands.com"      # optional override
//!
//! [services.archive]                       # present = enabled
//! # dir = "/path/to/archive"              # default: <data_dir>/recall/archive
//! ```
//!
//! [`Config::load`] silently migrates the older flat top-level `token` into
//! `services.seventeenlands.token` and rewrites the file. The CLI reads the token from
//! config only ([`resolve_token`]); without one it explains where to put it and exits.
//! No flags, no prompts, no env var.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Contents of `<config_dir>/recall/config.toml`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub services: Services,
}

/// One optional section per service; a present section means the service is enabled.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Services {
    pub seventeenlands: Option<SeventeenLandsConfig>,
    pub archive: Option<ArchiveConfig>,
}

/// `[services.seventeenlands]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeventeenLandsConfig {
    /// Client token from 17lands.com/account (UUID).
    pub token: Option<String>,
    /// Host override; `None` means [`crate::services::seventeenlands::DEFAULT_HOST`].
    pub host: Option<String>,
}

/// `[services.archive]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveConfig {
    /// Directory receiving one `<YYYY-MM-DD>.jsonl` file per day. `None` means
    /// [`default_archive_dir`].
    pub dir: Option<PathBuf>,
}

/// Default archive location: `<data_dir>/recall/archive` (`~/Library/Application
/// Support/recall/archive` on macOS, `~/.local/share/recall/archive` on Linux). Data, not
/// config - archives grow.
pub fn default_archive_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("recall").join("archive"))
}

impl ArchiveConfig {
    /// The configured directory, or the platform default.
    pub fn resolved_dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(default_archive_dir)
    }
}

/// On-disk shape: the current layout plus the legacy flat `token` key, so a pre-services
/// file parses and can be migrated.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    token: Option<String>,
    services: Services,
}

/// Config location: `<config_dir>/recall/config.toml` (`dirs::config_dir()`, which
/// respects `$XDG_CONFIG_HOME`).
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("recall").join("config.toml"))
}

impl Config {
    /// Load from [`config_path`], migrating a flat `token` and rewriting the file when
    /// that changed something. A missing or unreadable file yields `Config::default()`
    /// (the error is logged). Never call [`Config::save`] on the result of this without
    /// going through [`Config::load_strict`] first, or a typo in the file becomes data loss.
    pub fn load() -> Config {
        Self::load_strict().unwrap_or_else(|e| {
            log::error!("Ignoring unreadable config: {e}");
            Config::default()
        })
    }

    /// Like [`Config::load`], but an unparseable file is an error instead of defaults.
    /// Use this before any read-modify-write of the config. A missing file is still
    /// `Ok(Config::default())`.
    pub fn load_strict() -> io::Result<Config> {
        let path = config_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        let (cfg, changed) = read_raw(&path)?;
        if changed {
            log::info!("Migrating config to the per-service layout");
            cfg.save_to(&path)
                .unwrap_or_else(|e| log::error!("Failed to write config {}: {e}", path.display()));
        }
        Ok(cfg)
    }

    /// Load from an explicit path (flat-`token` migration applied in memory, nothing
    /// written). Missing or invalid file → `Config::default()`.
    pub fn load_from(path: &Path) -> Config {
        read_raw(path).unwrap_or_default().0
    }

    /// Save to [`config_path`], creating the directory.
    pub fn save(&self) -> io::Result<()> {
        let path = config_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        self.save_to(&path)
    }

    /// Save to an explicit path, creating parent directories.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let body = toml::to_string(self).map_err(io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)
    }

    /// The configured 17Lands token, if present and a valid UUID.
    pub fn seventeenlands_token(&self) -> Option<String> {
        let token = self.services.seventeenlands.as_ref()?.token.as_deref()?;
        validate_uuid_v4(token)
    }

    /// The 17Lands upload host: `services.seventeenlands.host`, else the live API.
    pub fn seventeenlands_host(&self) -> String {
        self.services
            .seventeenlands
            .as_ref()
            .and_then(|s| s.host.clone())
            .unwrap_or_else(|| crate::services::seventeenlands::DEFAULT_HOST.to_string())
    }

    /// Set `services.seventeenlands.token`, creating the section if needed.
    pub fn set_seventeenlands_token(&mut self, token: &str) {
        self.services
            .seventeenlands
            .get_or_insert_with(Default::default)
            .token = Some(token.to_string());
    }

    /// Names of the enabled services, primary first (`"seventeenlands"`, `"archive"`).
    pub fn service_names(&self) -> Vec<String> {
        let mut names = vec!["seventeenlands".to_string()];
        if self.services.archive.is_some() {
            names.push("archive".to_string());
        }
        names
    }
}

/// Parse the file at `path`; returns the config and whether the flat `token` key was
/// migrated (i.e. the file should be rewritten). A missing file is `Config::default()`;
/// a file that exists but doesn't parse is an error.
fn read_raw(path: &Path) -> io::Result<(Config, bool)> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Config::default(), false)),
        Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
    };
    let raw: RawConfig = toml::from_str(&contents).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })?;
    let mut cfg = Config {
        services: raw.services,
    };
    let mut changed = false;
    if let Some(flat) = raw.token.as_deref() {
        if cfg.seventeenlands_token().is_none()
            && let Some(valid) = validate_uuid_v4(flat)
        {
            cfg.set_seventeenlands_token(&valid);
        }
        changed = true; // the flat key is dropped either way
    }
    Ok((cfg, changed))
}

/// Validate a string is an acceptable UUID v4 token.
///
/// Mirrors Python's `uuid.UUID(s, version=4)` *acceptance set*, which is
/// lenient: it strips `urn:` / `uuid:` prefixes and surrounding braces, removes all
/// hyphens, then requires exactly 32 hexadecimal digits. The `version=4` argument only
/// rewrites version/variant bits afterward, so any well-formed UUID string is accepted
/// regardless of its actual version. Returns the *original* string on success (like the
/// Python helper, which returns `maybe_uuid` unchanged).
pub fn validate_uuid_v4(maybe: &str) -> Option<String> {
    // Python: hex = s.replace('urn:', '').replace('uuid:', '')
    let mut hex = maybe.replace("urn:", "").replace("uuid:", "");
    // Python: hex = hex.strip('{}')  - strip leading/trailing '{' and '}' chars.
    hex = hex
        .trim_start_matches(['{', '}'])
        .trim_end_matches(['{', '}'])
        .to_string();
    // Python: hex = hex.replace('-', '')
    hex = hex.replace('-', "");

    if hex.len() != 32 {
        return None;
    }
    // Python: int(hex, 16) - must be all hex digits.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(maybe.to_string())
}

/// Resolve the CLI token from `cfg`. Without one, print where the token goes (the config
/// path) and exit - the CLI never prompts; the desktop app has its own UI for this.
pub fn resolve_token(cfg: &Config) -> String {
    if let Some(t) = cfg.seventeenlands_token() {
        return t;
    }

    eprintln!("{}", missing_token_message(config_path().as_deref()));
    std::process::exit(1);
}

/// The message shown when no 17Lands token is configured.
fn missing_token_message(path: Option<&Path>) -> String {
    let location = path.map_or_else(
        || "<config_dir>/recall/config.toml".to_string(),
        |p| p.display().to_string(),
    );
    format!(
        "No 17Lands client token configured.\n\n\
         Get your token from https://www.17lands.com/account and add it to\n\
         {location}:\n\n\
         [services.seventeenlands]\n\
         token = \"00000000-0000-4000-8000-000000000000\""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "12345678-1234-4234-8234-123456789abc";

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load_from(&dir.path().join("nope.toml")),
            Config::default()
        );
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let cfg = Config {
            services: Services {
                seventeenlands: Some(SeventeenLandsConfig {
                    token: Some(TOKEN.into()),
                    host: Some("http://127.0.0.1:8732".into()),
                }),
                archive: Some(ArchiveConfig {
                    dir: Some("/tmp/recall-archive".into()),
                }),
            },
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), cfg);
        assert_eq!(cfg.seventeenlands_token().as_deref(), Some(TOKEN));
        assert_eq!(cfg.service_names(), ["seventeenlands", "archive"]);
    }

    #[test]
    fn flat_token_migrates_to_seventeenlands_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("token = \"{TOKEN}\"\n")).unwrap();
        let (cfg, changed) = read_raw(&path).unwrap();
        assert!(changed);
        assert_eq!(cfg.seventeenlands_token().as_deref(), Some(TOKEN));
        assert!(cfg.services.archive.is_none());
        // Re-saving writes the new layout only.
        cfg.save_to(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("[services.seventeenlands]"));
        assert!(!body.starts_with("token"));
        assert!(!read_raw(&path).unwrap().1);
    }

    #[test]
    fn unparseable_file_is_an_error_not_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[services.seventeenlands\ntoken = 1\n").unwrap();
        let err = read_raw(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("config.toml"));
        // The lenient loader falls back to defaults without touching the file.
        assert_eq!(Config::load_from(&path), Config::default());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("[services")
        );
        // A missing file is not an error.
        assert!(!read_raw(&dir.path().join("nope.toml")).unwrap().1);
    }

    #[test]
    fn missing_token_message_names_the_config_file() {
        let msg = missing_token_message(Some(Path::new("/tmp/recall/config.toml")));
        assert!(msg.contains("/tmp/recall/config.toml"));
        assert!(msg.contains("[services.seventeenlands]"));
        assert!(missing_token_message(None).contains("<config_dir>/recall/config.toml"));
    }

    #[test]
    fn accepts_standard_hyphenated_v4() {
        let t = "12345678-1234-4234-8234-123456789abc";
        assert_eq!(validate_uuid_v4(t).as_deref(), Some(t));
    }

    #[test]
    fn accepts_non_v4_uuid_like_python() {
        // Version nibble '1' (not 4) - Python overwrites it, so it's accepted.
        let t = "12345678-1234-1234-1234-123456789abc";
        assert_eq!(validate_uuid_v4(t).as_deref(), Some(t));
    }

    #[test]
    fn accepts_simple_32_hex_and_braces_and_urn() {
        assert!(validate_uuid_v4("123456781234423482341234567890ab").is_some());
        assert!(validate_uuid_v4("{12345678-1234-4234-8234-123456789abc}").is_some());
        assert!(validate_uuid_v4("urn:uuid:12345678-1234-4234-8234-123456789abc").is_some());
    }

    #[test]
    fn rejects_wrong_length_and_non_hex() {
        assert!(validate_uuid_v4("not-a-uuid").is_none());
        assert!(validate_uuid_v4("12345678-1234-4234-8234-123456789ab").is_none()); // 31 hex
        assert!(validate_uuid_v4("g2345678-1234-4234-8234-123456789abc").is_none()); // 'g'
        assert!(validate_uuid_v4("").is_none());
    }

    #[test]
    fn returns_original_unmodified_string() {
        // Preserves exact input (incl. case) like the Python helper.
        let t = "ABCDEF78-1234-4234-8234-123456789ABC";
        assert_eq!(validate_uuid_v4(t).as_deref(), Some(t));
    }

    #[test]
    fn bare_archive_section_enables_with_default_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[services.archive]\n").unwrap();
        let cfg = Config::load_from(&path);
        let archive = cfg.services.archive.clone().expect("section present");
        assert_eq!(archive.dir, None);
        assert_eq!(archive.resolved_dir(), default_archive_dir());
        assert_eq!(cfg.service_names(), ["seventeenlands", "archive"]);
    }
}
