// SPDX-License-Identifier: MIT

//! Checkpoint/resume tests: a run that is interrupted and resumed must hand the services
//! exactly the events one uninterrupted run would have - same payload bytes, same order,
//! none repeated. That is the whole claim of the muted replay (`follower/tail.rs`): the
//! state it rebuilds is the state the first run had.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use recall_core::checkpoint::Checkpoints;
use recall_core::follower::Follower;
use recall_core::services::seventeenlands::{
    RecordingSubmitter, SeventeenLands, SubmitOutcome, Submitter, to_python_json_string,
};

const TOKEN: &str = "00000000-0000-4000-8000-000000000000";

fn checkpoints(dir: &Path) -> Checkpoints {
    Checkpoints::load_from(Some(dir.join("state.json")), "test".into())
}

/// One run over `log` (read once, like `follow = false`), as a fresh process would do it:
/// new follower, checkpoints loaded from disk. Returns what was submitted.
fn run(log: &Path, state_dir: &Path) -> Vec<String> {
    run_with(log, state_dir, RecordingSubmitter::new())
        .calls
        .iter()
        .map(|c| format!("{} {}", c.endpoint, to_python_json_string(&c.payload)))
        .collect()
}

fn run_with<W: Submitter>(log: &Path, state_dir: &Path, wire: W) -> W {
    let mut cp = checkpoints(state_dir);
    let mut f = Follower::with_sink(SeventeenLands::new(TOKEN.into(), wire));
    let never = Arc::new(AtomicBool::new(false));
    f.parse_log_resumable(&log.to_string_lossy(), false, &never, Some(&mut cp));
    f.sink.wire
}

/// What a single uninterrupted run submits (no checkpoints involved).
fn full_run(fixture: &str) -> Vec<String> {
    let mut f = Follower::with_sink(SeventeenLands::new(TOKEN.into(), RecordingSubmitter::new()));
    f.parse_log(fixture, false);
    f.sink
        .wire
        .calls
        .iter()
        .map(|c| format!("{} {}", c.endpoint, to_python_json_string(&c.payload)))
        .collect()
}

/// Byte offsets just after each newline that starts a new log entry - the places a live
/// tail can realistically be caught up to (Arena writes whole entries).
fn entry_boundaries(data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, b) in data.iter().enumerate() {
        if *b == b'\n' {
            let start = i + 1;
            let rest = &data[start..];
            if rest.starts_with(b"[UnityCrossThreadLogger]") || rest.starts_with(b"[Client GRE]") {
                out.push(start);
            }
        }
    }
    out
}

/// Interrupt the log at `cut` (first run sees only the head), then let a second run see
/// the whole file; together they must equal the uninterrupted run.
fn assert_resume_matches(fixture: &str, cut: usize, expected: &[String]) {
    let data = std::fs::read(fixture).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("Player.log");

    std::fs::write(&log, &data[..cut]).unwrap();
    let mut got = run(&log, tmp.path());
    let first = got.len();

    std::fs::write(&log, &data).unwrap();
    got.extend(run(&log, tmp.path()));

    assert!(
        got == expected,
        "{fixture} cut at byte {cut}: {first} + {} submissions, expected {}",
        got.len() - first,
        expected.len()
    );

    // And a third run, with nothing new in the file, sends nothing.
    assert!(
        run(&log, tmp.path()).is_empty(),
        "{fixture} cut at {cut}: third run re-sent"
    );
}

#[test]
fn a_second_run_over_the_same_log_sends_nothing() {
    for fixture in ["tests/fixtures/gaps.log", "tests/fixtures/game.log"] {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("Player.log");
        std::fs::copy(fixture, &log).unwrap();

        assert_eq!(run(&log, tmp.path()), full_run(fixture));
        assert!(run(&log, tmp.path()).is_empty());
    }
}

#[test]
fn resuming_gaps_log_at_every_entry_matches_the_full_run() {
    let fixture = "tests/fixtures/gaps.log";
    let expected = full_run(fixture);
    let data = std::fs::read(fixture).unwrap();
    let cuts = entry_boundaries(&data);
    assert!(cuts.len() > 5);
    for cut in cuts {
        assert_resume_matches(fixture, cut, &expected);
    }
}

#[test]
fn resuming_game_log_mid_game_matches_the_full_run() {
    // A real match: resuming in the middle only works if the game state (objects, hands,
    // history, pending result) is rebuilt exactly.
    let fixture = "tests/fixtures/game.log";
    let expected = full_run(fixture);
    let data = std::fs::read(fixture).unwrap();
    let cuts = entry_boundaries(&data);
    assert!(cuts.len() > 100);
    for cut in cuts.iter().step_by(cuts.len() / 12) {
        assert_resume_matches(fixture, *cut, &expected);
    }
}

#[test]
fn a_rotated_log_resumes_under_its_new_name() {
    let fixture = "tests/fixtures/gaps.log";
    let expected = full_run(fixture);
    let data = std::fs::read(fixture).unwrap();
    let cut = entry_boundaries(&data)[5];
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("Player.log");
    let prev = tmp.path().join("Player-prev.log");

    // Arena was running; we processed part of Player.log.
    std::fs::write(&log, &data[..cut]).unwrap();
    let mut got = run(&log, tmp.path());
    // Arena wrote the rest, quit, relaunched: the old log is now Player-prev.log and a new
    // Player.log has started.
    std::fs::write(&prev, &data).unwrap();
    std::fs::write(&log, b"[UnityCrossThreadLogger]a brand new session\n").unwrap();

    got.extend(run(&prev, tmp.path()));
    assert_eq!(got, expected);
    assert!(run(&log, tmp.path()).is_empty());
}

/// Refuses the n-th upload (0-based), accepts the rest.
struct RejectNth {
    n: usize,
    endpoints: Vec<String>,
}

impl Submitter for RejectNth {
    fn submit(&mut self, endpoint: &str, _payload: Value, _use_gzip: bool) -> SubmitOutcome {
        self.endpoints.push(endpoint.to_string());
        if self.endpoints.len() - 1 == self.n {
            SubmitOutcome::Rejected { status: 401 }
        } else {
            SubmitOutcome::Accepted
        }
    }
}

#[test]
fn the_checkpoint_never_passes_an_undelivered_event() {
    let fixture = "tests/fixtures/gaps.log";
    let expected = full_run(fixture);
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("Player.log");
    std::fs::copy(fixture, &log).unwrap();

    // The 4th upload is refused. Everything is still attempted in this run...
    let wire = run_with(
        &log,
        tmp.path(),
        RejectNth {
            n: 3,
            endpoints: Vec::new(),
        },
    );
    assert_eq!(wire.endpoints.len(), expected.len());

    // ...but the next run offers the refused event, and everything after it, again.
    assert_eq!(run(&log, tmp.path()), expected[3..]);
}
