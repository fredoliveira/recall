// SPDX-License-Identifier: MIT

//! JSONL archive service: appends every event, in its neutral JSON view, to a local file.
//!
//! Why it exists: you own your own data (a service-neutral record of every event the
//! client ever parsed, on disk, regardless of which services are configured), and the
//! archive doubles as the input for offline replay and fixtures.
//!
//! Layout: one file per local calendar day, `<dir>/<YYYY-MM-DD>.jsonl`, chosen by the
//! event's log-line timestamp ([`Context::log_time`](crate::events::Context)) rather
//! than the wall clock, so replaying an old log archives under the dates the events
//! happened and the output is deterministic. Each line is one compact JSON object:
//!
//! ```json
//! {"v": 1, "event": "<GameEvent::name()>", ...GameEvent::to_json() fields...}
//! ```
//!
//! `v` is the archive format version; bump [`FORMAT_VERSION`] if the neutral event shape
//! ever changes incompatibly.
//!
//! Secondary-service rules: a failed open or write is logged (WARN once, then DEBUG) and
//! the event is dropped; the service never panics and cannot affect the 17Lands leg.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::NaiveDate;
use serde_json::{Map, Value};

use crate::events::{GameEvent, Service};

/// Archive line format version (`"v"`).
pub const FORMAT_VERSION: u64 = 1;

/// A [`Service`] that appends each event as one JSON line to `<dir>/<YYYY-MM-DD>.jsonl`.
pub struct Archive {
    dir: PathBuf,
    open: Option<(NaiveDate, File)>,
    warned: bool,
}

impl Archive {
    /// Archive into `dir` (created, along with the day files, on first write).
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            open: None,
            warned: false,
        }
    }

    /// Path of the day file an event with this log date lands in.
    pub fn path_for(&self, date: NaiveDate) -> PathBuf {
        self.dir.join(format!("{}.jsonl", date.format("%Y-%m-%d")))
    }

    /// One archive line (without the trailing newline).
    pub fn line(event: &GameEvent) -> Vec<u8> {
        let mut m = Map::new();
        m.insert("v".into(), Value::from(FORMAT_VERSION));
        m.insert("event".into(), Value::from(event.name()));
        if let Value::Object(fields) = event.to_json() {
            m.extend(fields);
        }
        serde_json::to_vec(&Value::Object(m)).expect("serde_json::Value always serializes")
    }

    fn append(&mut self, date: NaiveDate, line: &[u8]) -> std::io::Result<()> {
        if !matches!(&self.open, Some((d, _)) if *d == date) {
            self.open = None;
            std::fs::create_dir_all(&self.dir)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path_for(date))?;
            self.open = Some((date, file));
        }
        let file = &mut self.open.as_mut().expect("just opened").1;
        file.write_all(line)?;
        file.write_all(b"\n")
    }
}

impl Service for Archive {
    fn handle(&mut self, event: &GameEvent) {
        let date = event.context().log_time.date();
        let line = Self::line(event);
        match self.append(date, &line) {
            Ok(()) => self.warned = false,
            Err(e) if self.warned => log::debug!("Archive write failed: {e}"),
            Err(e) => {
                log::warn!("Archive write failed ({}): {e}", self.dir.display());
                self.warned = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Context;
    use chrono::Days;

    fn event(ctx: Context) -> GameEvent {
        GameEvent::EventEnded {
            event_name: Some("PremierDraft_XYZ".into()),
            ctx,
        }
    }

    #[test]
    fn writes_one_line_per_event_in_the_day_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut archive = Archive::new(tmp.path().join("archive"));
        archive.handle(&event(Context::test_fixture()));
        archive.handle(&event(Context::test_fixture()));

        let body = std::fs::read_to_string(tmp.path().join("archive/2024-02-02.jsonl")).unwrap();
        let lines: Vec<Value> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        let keys: Vec<_> = lines[0].as_object().unwrap().keys().cloned().collect();
        assert_eq!(&keys[..2], ["v", "event"]);
        assert_eq!(lines[0]["v"], 1);
        assert_eq!(lines[0]["event"], "event_ended");
        assert_eq!(lines[0]["player_id"], "P1");
        assert_eq!(lines[0]["time"], "2024-02-02T13:00:00");
        assert_eq!(lines[0]["event_name"], "PremierDraft_XYZ");
    }

    #[test]
    fn different_days_land_in_different_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut archive = Archive::new(tmp.path().to_path_buf());
        let mut later = Context::test_fixture();
        later.log_time = later.log_time.checked_add_days(Days::new(1)).unwrap();
        archive.handle(&event(Context::test_fixture()));
        archive.handle(&event(later));
        archive.handle(&event(Context::test_fixture()));

        let count = |name: &str| {
            std::fs::read_to_string(tmp.path().join(name))
                .unwrap()
                .lines()
                .count()
        };
        assert_eq!(count("2024-02-02.jsonl"), 2);
        assert_eq!(count("2024-02-03.jsonl"), 1);
    }

    #[test]
    fn unwritable_dir_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, b"").unwrap();
        let mut archive = Archive::new(file.join("archive"));
        archive.handle(&event(Context::test_fixture()));
        archive.handle(&event(Context::test_fixture()));
        assert!(archive.warned);
    }
}
