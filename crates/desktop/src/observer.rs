//! Upload observability via the core's `Submitter` trait.
//!
//! Every 17Lands upload funnels through `Submitter::submit`. By wrapping the real
//! [`ApiClient`] in a decorator and passing it as the 17Lands leg's wire to
//! `services::from_config` (then `Follower::with_sink`), we capture structured upload status
//! (endpoint, time, accepted/failed counts) without touching - or parsing the logs of - the
//! wire-critical core.
//! Secondary services (the archive) never see it.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use recall_core::services::seventeenlands::{ApiClient, SubmitOutcome, Submitter};

/// Snapshot of upload activity, shared with the UI.
#[derive(Default, Clone)]
pub struct UploadStatus {
    /// Uploads 17Lands accepted.
    pub count: u64,
    pub last_endpoint: Option<String>,
    pub last_time: Option<String>,
    /// Uploads that were rejected or never delivered - each one an event missing from 17Lands.
    pub failed: u64,
    /// e.g. "add_game: HTTP 400 @ 21:04:11"
    pub last_failure: Option<String>,
}

/// A `Submitter` that delegates to the live client and records what it sent.
pub struct ObservingSubmitter {
    inner: ApiClient,
    status: Arc<Mutex<UploadStatus>>,
    /// Called after each recorded outcome, so the UI refreshes without polling.
    on_change: Box<dyn Fn() + Send>,
}

impl ObservingSubmitter {
    pub fn new(inner: ApiClient, status: Arc<Mutex<UploadStatus>>, on_change: Box<dyn Fn() + Send>) -> Self {
        Self { inner, status, on_change }
    }
}

impl Submitter for ObservingSubmitter {
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) -> SubmitOutcome {
        // Delegate the actual POST verbatim - payload behavior is unchanged.
        let outcome = self.inner.submit(endpoint, payload, use_gzip);

        if let Ok(mut s) = self.status.lock() {
            let now = chrono::Local::now().format("%H:%M:%S").to_string();
            let short = endpoint.rsplit('/').next().unwrap_or(endpoint);
            match outcome {
                SubmitOutcome::Accepted => {
                    s.count += 1;
                    s.last_endpoint = Some(endpoint.to_string());
                    s.last_time = Some(now);
                }
                SubmitOutcome::Rejected { status } => {
                    s.failed += 1;
                    s.last_failure = Some(format!("{short}: HTTP {status} @ {now}"));
                }
                // A stop, not a lost event: the next start uploads it.
                SubmitOutcome::Cancelled => {}
                SubmitOutcome::Failed => {
                    s.failed += 1;
                    s.last_failure = Some(format!("{short}: not delivered @ {now}"));
                }
            }
        }
        // After the lock is released: the listener reads the status back.
        (self.on_change)();
        outcome
    }
}
