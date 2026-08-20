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
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool);
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
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) {
        let url = format!("{}/{}", self.host, endpoint);
        let json_bytes = to_python_json_vec(&payload);

        let body: Vec<u8> = if use_gzip {
            match gzip_compress(&json_bytes) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("Failed to gzip payload for {endpoint}: {e}");
                    return;
                }
            }
        } else {
            json_bytes
        };

        let result = crate::retry::retry_api_call(
            || send_post(&url, &body, use_gzip),
            |resp: &ureq::Response| {
                // Retry only 5xx: valid when status is outside 500..600.
                !(500..600).contains(&resp.status())
            },
        );

        match result {
            Ok(resp) => {
                log::debug!("{endpoint} -> {}", resp.status());
            }
            Err(crate::retry::RetryError::LimitExceeded) => {
                log::error!("Giving up on {endpoint} after exhausting retries (server 5xx)");
            }
            Err(crate::retry::RetryError::Fatal(e)) => {
                log::error!("Transport error posting to {endpoint}: {e}");
            }
        }
    }
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
    fn submit(&mut self, endpoint: &str, payload: Value, use_gzip: bool) {
        self.calls.push(RecordedCall {
            endpoint: endpoint.to_string(),
            payload,
            use_gzip,
        });
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
