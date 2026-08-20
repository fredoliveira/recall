//! Upload observability via the core's `Submitter` trait.
//!
//! Every 17Lands upload funnels through `Submitter::submit`. By wrapping the real
//! [`ApiClient`] in a decorator and passing it as the 17Lands leg's wire to
//! `services::from_config` (then `Follower::with_sink`), we capture structured upload status
//! (endpoint, time, count) without touching - or parsing the logs of - the wire-critical core.
//! Secondary services (the archive) never see it.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use recall_core::services::seventeenlands::{ApiClient, Submitter};

/// Snapshot of upload activity, shared with the UI/tray.
#[derive(Default, Clone, Serialize)]
pub struct UploadStatus {
    pub count: u64,
    pub last_endpoint: Option<String>,
    pub last_time: Option<String>,
}

/// A `Submitter` that delegates to the live client and records what it sent.
pub struct ObservingSubmitter {
    inner: ApiClient,
    status: Arc<Mutex<UploadStatus>>,
}

impl ObservingSubmitter {
    pub fn new(inner: ApiClient, status: Arc<Mutex<UploadStatus>>) -> Self {
        Self { inner, status }
    }
}

impl Submitter for ObservingSubmitter {
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) {
        // Delegate the actual POST verbatim - payload behavior is unchanged.
        self.inner.submit(endpoint, payload, use_gzip);

        if let Ok(mut s) = self.status.lock() {
            s.count += 1;
            s.last_endpoint = Some(endpoint.to_string());
            s.last_time = Some(chrono::Local::now().format("%H:%M:%S").to_string());
        }
    }
}
