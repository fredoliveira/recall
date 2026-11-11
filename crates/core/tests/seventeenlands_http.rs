// SPDX-License-Identifier: MIT

//! Integration tests for the 17Lands transport (`services::seventeenlands::ApiClient`) HTTP path.
//!
//! Drives the real client against a loopback `TcpListener` mock so we exercise ureq,
//! header/body construction, gzip, and the retry/normalization layer end-to-end without
//! touching the network.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use recall_core::services::seventeenlands::{
    ApiClient, SubmitOutcome, Submitter, to_python_json_vec,
};

/// A captured HTTP request.
struct CapturedRequest {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Bind a loopback server that accepts one connection, captures the request, and replies
/// with each of `responses` in turn (one per connection). Returns `(host_url, handle)`,
/// where the handle yields the captured requests once joined.
fn spawn_mock(responses: Vec<&'static str>) -> (String, thread::JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let host = format!("http://127.0.0.1:{port}");

    let handle = thread::spawn(move || {
        let mut captured = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            captured.push(read_request(&mut stream));
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
        captured
    });

    (host, handle)
}

/// Read one HTTP request: status line, headers, and a `Content-Length`-delimited body.
fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];

    // Read until the end of headers.
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut tmp).unwrap();
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).unwrap();
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }

    CapturedRequest {
        request_line,
        headers,
        body,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

const RESP_200: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

#[test]
fn posts_plain_json_with_expected_method_path_and_body() {
    let (host, handle) = spawn_mock(vec![RESP_200]);
    let mut client = ApiClient::new(host);

    let payload = json!({"token": "abc", "player_id": "p1", "screen_name": "Tester"});
    let outcome = client.submit("api/client/add_mtga_account", payload.clone(), false);
    assert_eq!(outcome, SubmitOutcome::Accepted);

    let reqs = handle.join().unwrap();
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];

    assert_eq!(
        req.request_line,
        "POST /api/client/add_mtga_account HTTP/1.1"
    );
    assert_eq!(req.header("Content-Type"), Some("application/json"));
    assert!(
        req.header("Content-Encoding").is_none(),
        "plain POST must not be gzipped"
    );
    // Body is byte-identical to Python `json.dumps(payload)`.
    assert_eq!(req.body, to_python_json_vec(&payload));
}

#[test]
fn posts_gzipped_game_result_with_content_encoding() {
    let (host, handle) = spawn_mock(vec![RESP_200]);
    let mut client = ApiClient::new(host);

    let payload = json!({"match_id": "m1", "won": true, "turns": 9});
    client.submit("api/client/add_game", payload.clone(), true);

    let reqs = handle.join().unwrap();
    let req = &reqs[0];

    assert_eq!(req.request_line, "POST /api/client/add_game HTTP/1.1");
    assert_eq!(req.header("Content-Type"), Some("application/json"));
    assert_eq!(req.header("Content-Encoding"), Some("gzip"));

    // Decompress and confirm it matches the Python-style JSON bytes.
    let mut decoder = flate2::read::GzDecoder::new(&req.body[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).unwrap();
    assert_eq!(decompressed, to_python_json_vec(&payload));
}

#[test]
fn does_not_retry_on_4xx() {
    // A 404 is "valid" per the response validator (status < 500), so exactly one request
    // is made - no retry. (If it retried, the mock would block on a 2nd accept and the
    // join below would hang; one queued response proves single-shot.)
    let (host, handle) = spawn_mock(vec!["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"]);
    let mut client = ApiClient::new(host);

    let outcome = client.submit("api/client/add_rank", json!({"rank_data": null}), false);
    assert_eq!(outcome, SubmitOutcome::Rejected { status: 404 });

    let reqs = handle.join().unwrap();
    assert_eq!(reqs.len(), 1, "4xx must not trigger a retry");
    assert_eq!(reqs[0].request_line, "POST /api/client/add_rank HTTP/1.1");
}

#[test]
fn app_level_403_is_rejected_without_retry() {
    // A 403 the app itself returns (no `Server: awselb`) is a real rejection, not throttling.
    let (host, handle) = spawn_mock(vec![
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    ]);
    let mut client = ApiClient::new(host);

    let outcome = client.submit("api/client/add_rank", json!({"rank_data": null}), false);
    assert_eq!(outcome, SubmitOutcome::Rejected { status: 403 });
    assert_eq!(handle.join().unwrap().len(), 1);
}

#[test]
fn retries_429_until_accepted() {
    let (host, handle) = spawn_mock(vec![
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        RESP_200,
    ]);
    let mut client = ApiClient::new(host);

    let payload = json!({"match_id": "m1"});
    let outcome = client.submit("api/client/add_game", payload.clone(), true);
    assert_eq!(outcome, SubmitOutcome::Accepted);

    let reqs = handle.join().unwrap();
    assert_eq!(reqs.len(), 2, "429 must be retried, not dropped");
    assert_eq!(
        reqs[0].body, reqs[1].body,
        "the retry resends the same event"
    );
}

#[test]
fn retries_load_balancer_firewall_403_until_accepted() {
    // What 17Lands' AWS load balancer returns to a rate-limited IP.
    let (host, handle) = spawn_mock(vec![
        "HTTP/1.1 403 Forbidden\r\nServer: awselb/2.0\r\nContent-Type: text/html\r\n\
         Content-Length: 9\r\nConnection: close\r\n\r\nForbidden",
        RESP_200,
    ]);
    let mut client = ApiClient::new(host);

    let outcome = client.submit("api/client/add_rank", json!({"rank_data": null}), false);
    assert_eq!(outcome, SubmitOutcome::Accepted);
    assert_eq!(
        handle.join().unwrap().len(),
        2,
        "firewall 403 must be retried"
    );
}

#[test]
fn consecutive_uploads_reuse_one_connection() {
    // A server that accepts exactly one connection and answers two requests on it: the
    // second submit only succeeds if the client kept the connection alive.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drop(listener);
        (0..2)
            .map(|_| {
                let req = read_request(&mut stream);
                stream.write_all(RESP_200.as_bytes()).unwrap();
                stream.flush().unwrap();
                req
            })
            .collect::<Vec<_>>()
    });

    let mut client = ApiClient::new(host);
    for endpoint in ["api/client/add_pack", "api/client/add_pick"] {
        let outcome = client.submit(endpoint, json!({"token": "abc"}), false);
        assert_eq!(outcome, SubmitOutcome::Accepted);
    }

    let reqs = handle.join().unwrap();
    assert_eq!(reqs[0].request_line, "POST /api/client/add_pack HTTP/1.1");
    assert_eq!(reqs[1].request_line, "POST /api/client/add_pick HTTP/1.1");
}

#[test]
fn cancelling_ends_a_rate_limit_backoff() {
    // One 429, then nothing: without the cancel flag the client would sleep and retry.
    let (host, handle) = spawn_mock(vec![
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ]);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut client = ApiClient::new(host).with_cancel(cancel.clone());

    let setter = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let outcome = client.submit("api/client/add_rank", json!({"rank_data": null}), false);
    setter.join().unwrap();

    assert_eq!(outcome, SubmitOutcome::Cancelled);
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "returned before the 1s backoff ran out"
    );
    assert_eq!(handle.join().unwrap().len(), 1);
}
