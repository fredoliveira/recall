// SPDX-License-Identifier: MIT

//! Wire transport for the 17Lands API (port of upstream `api_client.py`'s HTTP layer).
//!
//! - Receives fully-enveloped payloads from the 17Lands service adapter
//!   ([`super::SeventeenLands`], which owns the envelope and endpoint routing) and posts
//!   them; this module only moves bytes.
//! - Only `add_game` is gzipped in the kept endpoint subset.
//! - `client_version_validation`, `log_errors`, `add_event` are dropped.
//!
//! ## JSON serialization
//! Python POSTs via `requests`, whose body is `json.dumps(blob)` with the default
//! `(", ", ": ")` separators. We serialize with a matching formatter so ASCII payloads are
//! byte-identical to the Python client. (Python also defaults to `ensure_ascii=True`,
//! \u-escaping non-ASCII; we emit raw UTF-8 instead - still valid JSON the server parses
//! identically.)

#![allow(dead_code)]

use std::io::Write;

use serde::Serialize;
use serde_json::Value;

/// The wire seam: performs (or records / decorates) a POST of a fully-enveloped payload.
///
/// Endpoint routing and envelope construction live one level up, in
/// [`super::SeventeenLands`]; implementations here only move
/// bytes ([`ApiClient`] over HTTP, [`RecordingSubmitter`] into a Vec for the parity
/// tests, the desktop's observer as a decorator).
pub trait Submitter {
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) -> SubmitOutcome;
}

/// What became of one submission. Failures are already logged by the client; the outcome
/// exists for observers (the desktop status), and the follower carries on regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// 2xx, possibly after retries.
    Accepted,
    /// The server answered non-2xx and wasn't retried: the event was not uploaded.
    Rejected { status: u16 },
    /// Never answered: transport error, gzip failure, or retries exhausted.
    Failed,
}

/// The live REST client (port of `api_client.ApiClient`).
pub struct ApiClient {
    pub host: String,
}

impl ApiClient {
    pub fn new(host: impl Into<String>) -> Self {
        Self { host: host.into() }
    }
}

impl Submitter for ApiClient {
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) -> SubmitOutcome {
        let url = format!("{}/{}", self.host, endpoint);
        let json_bytes = to_python_json_vec(&payload);

        let body: Vec<u8> = if use_gzip {
            match gzip_compress(&json_bytes) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("Failed to gzip payload for {endpoint}: {e}");
                    return SubmitOutcome::Failed;
                }
            }
        } else {
            json_bytes
        };

        let mut throttled = 0u32;
        let result = crate::retry::retry_api_call(
            || -> Result<ureq::Response, Box<ureq::Transport>> {
                let resp = send_post(&url, &body, use_gzip)?;
                if is_throttled(&resp) {
                    throttled += 1;
                    // Warn once per submission; the backoff can run for hours.
                    if throttled == 1 {
                        log::warn!(
                            "17Lands is rate-limiting uploads ({endpoint} -> {}); \
                             holding this event and retrying with backoff",
                            resp.status()
                        );
                    } else {
                        log::debug!("{endpoint} still rate-limited (attempt {throttled})");
                    }
                }
                Ok(resp)
            },
            |resp: &ureq::Response| !(is_server_error(resp) || is_throttled(resp)),
        );

        match result {
            Ok(resp) if (200..300).contains(&resp.status()) => {
                if throttled > 0 {
                    log::info!("{endpoint} accepted after {throttled} rate-limited attempt(s)");
                }
                log::debug!("{endpoint} -> {}", resp.status());
                SubmitOutcome::Accepted
            }
            Ok(resp) => {
                let status = resp.status();
                log::warn!(
                    "17Lands rejected {endpoint} (HTTP {status}{}); this event was not uploaded",
                    body_excerpt(resp)
                        .map(|b| format!(": {b}"))
                        .unwrap_or_default()
                );
                SubmitOutcome::Rejected { status }
            }
            Err(crate::retry::RetryError::LimitExceeded) => {
                log::error!(
                    "Giving up on {endpoint} after exhausting retries (server 5xx or rate limit)"
                );
                SubmitOutcome::Failed
            }
            Err(crate::retry::RetryError::Fatal(e)) => {
                log::error!("Transport error posting to {endpoint}: {e}");
                SubmitOutcome::Failed
            }
        }
    }
}

fn is_server_error(resp: &ureq::Response) -> bool {
    (500..600).contains(&resp.status())
}

/// 429, or a 403 the AWS load balancer generated itself: 17Lands' rate-limit firewall
/// answers every request from a throttled IP that way (`Server: awselb/…`, HTML body)
/// until the rate drops. A 403 from the app has no `awselb` server header and is a real
/// rejection. Deliberate deviation: upstream retries only 5xx and drops these events.
fn is_throttled(resp: &ureq::Response) -> bool {
    resp.status() == 429
        || (resp.status() == 403
            && resp
                .header("server")
                .is_some_and(|s| s.to_ascii_lowercase().starts_with("awselb")))
}

/// The first line of an error body (e.g. the server's reason for a 400), trimmed for a log line.
fn body_excerpt(resp: ureq::Response) -> Option<String> {
    let text = resp.into_string().ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(200).collect())
}

/// Send one POST. Non-2xx HTTP responses are normalized from `Err(Status)` back to
/// `Ok(Response)` so the retry layer's `response_validator` sees the status; only genuine
/// transport failures return `Err` (see the `retry` module's ureq normalization note).
fn send_post(
    url: &str,
    body: &[u8],
    use_gzip: bool,
) -> Result<ureq::Response, Box<ureq::Transport>> {
    let mut req = ureq::post(url).set("Content-Type", "application/json");
    if use_gzip {
        req = req.set("Content-Encoding", "gzip");
    }

    match req.send_bytes(body) {
        Ok(resp) => Ok(resp),
        // A real HTTP response (4xx/5xx) - hand it to the validator, don't treat as transport error.
        Err(ureq::Error::Status(_code, resp)) => Ok(resp),
        // Box the transport error: ureq's `Transport` is large, and it's the cold path.
        Err(ureq::Error::Transport(t)) => Err(Box::new(t)),
    }
}

/// gzip a byte slice (default compression; matches Python `gzip.compress` semantically -
/// the server decompresses, so compression level / mtime header bytes are immaterial).
fn gzip_compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

/// Serialize a `Value` exactly like Python `json.dumps(blob)` with its default
/// `(", ", ": ")` separators, so ASCII payload bytes match the Python client.
// WIRE: Python `json.dumps` default separators + insertion order (`preserve_order`).
pub fn to_python_json_vec(value: &Value) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PythonFormatter);
    value
        .serialize(&mut ser)
        .expect("serializing a serde_json::Value to a Vec never fails");
    buf
}

/// Convenience wrapper returning a `String`.
pub fn to_python_json_string(value: &Value) -> String {
    String::from_utf8(to_python_json_vec(value)).expect("python-json output is valid UTF-8")
}

/// A `serde_json` formatter matching Python `json.dumps`'s default separators: `", "`
/// between items and `": "` between a key and its value. All other behavior falls back to
/// the compact defaults provided by the `Formatter` trait.
struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        writer.write_all(b": ")
    }
}

// ---------------------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------------------

/// A single recorded submission (test mock).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    pub endpoint: String,
    pub payload: Value,
    pub use_gzip: bool,
}

/// A [`Submitter`] that records `(endpoint, payload, use_gzip)` instead of sending, for the
/// fixture parity tests. Lives in `src` (not `#[cfg(test)]`) so both unit
/// tests and the `tests/parity.rs` integration harness share one definition.
#[derive(Debug, Default)]
pub struct RecordingSubmitter {
    pub calls: Vec<RecordedCall>,
}

impl RecordingSubmitter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Submitter for RecordingSubmitter {
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) -> SubmitOutcome {
        self.calls.push(RecordedCall {
            endpoint: endpoint.to_string(),
            payload,
            use_gzip,
        });
        SubmitOutcome::Accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_json_matches_default_separators() {
        let v = json!({"a": 1, "b": [1, 2, 3], "c": null, "d": {"e": true}});
        // Exactly what Python `json.dumps(...)` produces with default separators.
        let expected = r#"{"a": 1, "b": [1, 2, 3], "c": null, "d": {"e": true}}"#;
        assert_eq!(to_python_json_string(&v), expected);
    }

    #[test]
    fn python_json_empty_containers() {
        assert_eq!(to_python_json_string(&json!({})), "{}");
        assert_eq!(to_python_json_string(&json!([])), "[]");
        assert_eq!(to_python_json_string(&json!({"x": []})), r#"{"x": []}"#);
    }

    #[test]
    fn gzip_roundtrips() {
        use std::io::Read;
        let original = br#"{"hello": "world"}"#;
        let compressed = gzip_compress(original).unwrap();
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap();
        assert_eq!(out, original);
    }
}
