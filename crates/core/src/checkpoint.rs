// SPDX-License-Identifier: MIT

//! Checkpoints: remember how far into a log the services have already been fed, so a
//! restart does not re-upload the whole file.
//!
//! The follower cannot simply seek to a saved offset - its state (player id, draft, game in
//! progress) is built from everything before it. So a checkpoint is a *replay target*: on
//! start the tail loop re-reads the log from byte 0 with the services muted, which rebuilds
//! the state exactly and submits nothing, and goes live once it reaches the checkpoint
//! (see `follower/tail.rs`).
//!
//! A checkpoint is `(offset, flushed)` plus a fingerprint of the bytes just before the
//! offset. Arena rewrites `Player.log` on every launch (the old one becomes
//! `Player-prev.log`), and the head of every log is the same engine boilerplate, so the
//! fingerprint is taken at the *tail*: that is where the timestamps and ids are. A
//! checkpoint applies to a file when the file is at least `offset` long and those bytes
//! still hash the same - whatever the file is called now, which is what makes a rotated
//! `Player-prev.log` resume from the checkpoint written while it was `Player.log`.
//!
//! Checkpoints are scoped to where the events went (host + token, hashed): pointing the
//! client at a mock server, or at another account, must not mark events as delivered to
//! the real one.
//!
//! Not part of the wire contract. Failures here are logged and otherwise ignored - the
//! worst case is today's behaviour, a full re-upload.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// State file format version (`"v"`).
const FORMAT_VERSION: u64 = 1;
/// How many bytes before the offset are fingerprinted.
const TAIL_WINDOW: u64 = 4096;

/// Where to stop the muted replay and go live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume {
    /// Bytes `[0, offset)` have been delivered already.
    pub offset: u64,
    /// Whether the entry buffer had been flushed at `offset` (the checkpoint was taken at
    /// an end-of-file flush rather than after a line), so the replay must flush there too.
    pub flushed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    scope: String,
    log: String,
    offset: u64,
    flushed: bool,
    tail_len: u64,
    tail_hash: String,
}

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    v: u64,
    #[serde(default)]
    checkpoints: Vec<Entry>,
}

/// The checkpoint store: one entry per `(scope, log path)`, persisted as JSON.
pub struct Checkpoints {
    path: Option<PathBuf>,
    scope: String,
    entries: Vec<Entry>,
    warned: bool,
}

/// Default location: `<data_dir>/recall/state.json`.
pub fn default_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("recall").join("state.json"))
}

/// The scope for a 17Lands destination: a hash of host and token (never the token itself).
pub fn scope_for(host: &str, token: &str) -> String {
    let mut h = Fnv::new();
    h.write(host.as_bytes());
    h.write(b"\n");
    h.write(token.as_bytes());
    h.hex()
}

impl Checkpoints {
    /// Load the store at [`default_path`] for `scope`.
    pub fn load(scope: String) -> Self {
        Self::load_from(default_path(), scope)
    }

    /// Load the store at an explicit path. A missing, unreadable or foreign-version file is
    /// an empty store; `None` gives a store that never persists.
    pub fn load_from(path: Option<PathBuf>, scope: String) -> Self {
        let entries = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<StateFile>(&bytes).ok())
            .filter(|f| f.v == FORMAT_VERSION)
            .map(|f| f.checkpoints)
            .unwrap_or_default();
        Self {
            path,
            scope,
            entries,
            warned: false,
        }
    }

    /// The furthest checkpoint in this scope that matches `log` as it is on disk right now.
    pub fn resume_point(&self, log: &Path) -> Option<Resume> {
        let len = std::fs::metadata(log).ok()?.len();
        self.entries
            .iter()
            .filter(|e| e.scope == self.scope && e.offset > 0 && e.offset <= len)
            .filter(|e| tail_hash(log, e.offset, e.tail_len).is_ok_and(|hash| hash == e.tail_hash))
            .max_by_key(|e| e.offset)
            .map(|e| Resume {
                offset: e.offset,
                flushed: e.flushed,
            })
    }

    /// Record that everything in `log` before `offset` has been delivered, and persist.
    pub fn record(&mut self, log: &Path, offset: u64, flushed: bool) {
        let tail_len = offset.min(TAIL_WINDOW);
        let result = tail_hash(log, offset, tail_len).and_then(|tail_hash| {
            let log = log.to_string_lossy().to_string();
            self.entries
                .retain(|e| !(e.scope == self.scope && e.log == log));
            self.entries.push(Entry {
                scope: self.scope.clone(),
                log,
                offset,
                flushed,
                tail_len,
                tail_hash,
            });
            self.save()
        });
        match result {
            Ok(()) => self.warned = false,
            Err(e) if self.warned => log::debug!("Checkpoint write failed: {e}"),
            Err(e) => {
                log::warn!(
                    "Could not save the log checkpoint ({e}); a restart will re-upload this log"
                );
                self.warned = true;
            }
        }
    }

    /// Write the store atomically (temp file + rename), so a crash never leaves half a file.
    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec(&StateFile {
            v: FORMAT_VERSION,
            checkpoints: self.entries.clone(),
        })
        .map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    }
}

/// Hash of the `len` bytes of `log` that end at `offset`.
fn tail_hash(log: &Path, offset: u64, len: u64) -> std::io::Result<String> {
    let mut file = File::open(log)?;
    file.seek(SeekFrom::Start(offset.saturating_sub(len)))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)?;
    let mut h = Fnv::new();
    h.write(&buf);
    Ok(h.hex())
}

/// FNV-1a, 64-bit. Hand-rolled because the value is persisted: `std`'s `DefaultHasher`
/// makes no promise of producing the same output across Rust versions.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path, scope: &str) -> Checkpoints {
        Checkpoints::load_from(Some(dir.join("state.json")), scope.into())
    }

    #[test]
    fn round_trips_through_the_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        std::fs::write(&log, b"line one\nline two\nline three\n").unwrap();

        let mut cp = store(tmp.path(), "s");
        assert_eq!(cp.resume_point(&log), None);
        cp.record(&log, 18, true);

        let resume = Resume {
            offset: 18,
            flushed: true,
        };
        assert_eq!(cp.resume_point(&log), Some(resume));
        assert_eq!(store(tmp.path(), "s").resume_point(&log), Some(resume));
    }

    #[test]
    fn a_rewritten_log_does_not_match() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        std::fs::write(&log, b"session one, entry A\n").unwrap();
        let mut cp = store(tmp.path(), "s");
        cp.record(&log, 21, false);

        // Shorter than the offset.
        std::fs::write(&log, b"new\n").unwrap();
        assert_eq!(cp.resume_point(&log), None);
        // Long enough, different bytes.
        std::fs::write(&log, b"session two, entry B\nmore\n").unwrap();
        assert_eq!(cp.resume_point(&log), None);
    }

    #[test]
    fn a_rotated_log_matches_under_its_new_name() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        let prev = tmp.path().join("Player-prev.log");
        std::fs::write(&log, b"session one, entry A\n").unwrap();
        let mut cp = store(tmp.path(), "s");
        cp.record(&log, 21, false);

        std::fs::rename(&log, &prev).unwrap();
        assert_eq!(cp.resume_point(&prev).map(|r| r.offset), Some(21));
    }

    #[test]
    fn scopes_do_not_see_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        std::fs::write(&log, b"an entry\n").unwrap();
        store(tmp.path(), "mock").record(&log, 9, false);

        assert_eq!(store(tmp.path(), "live").resume_point(&log), None);
        assert!(store(tmp.path(), "mock").resume_point(&log).is_some());
        assert_ne!(scope_for("a", "t"), scope_for("b", "t"));
        assert_ne!(scope_for("a", "t"), scope_for("a", "u"));
    }

    #[test]
    fn recording_keeps_other_scopes_and_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        std::fs::write(&log, b"an entry\nanother\n").unwrap();
        store(tmp.path(), "mock").record(&log, 9, false);
        store(tmp.path(), "live").record(&log, 17, false);

        assert_eq!(
            store(tmp.path(), "mock")
                .resume_point(&log)
                .map(|r| r.offset),
            Some(9)
        );
        assert_eq!(
            store(tmp.path(), "live")
                .resume_point(&log)
                .map(|r| r.offset),
            Some(17)
        );
    }
}
