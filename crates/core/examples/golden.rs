// SPDX-License-Identifier: MIT

//! Golden-snapshot harness over real (gitignored) Player.logs - the refactor safety net.
//!
//! Replays a log through the Follower with a recording submitter and serializes every
//! would-be submission as one python-JSON line. First run writes the baseline; later runs
//! must reproduce it byte-for-byte or this exits non-zero with the first divergence.
//! Baselines contain real account data: keep them under `local/` (gitignored).
//!
//! Usage: `cargo run -p recall-core --example golden -- <logfile> <golden.jsonl>`
//! (or `just golden` to sweep `local/logs/*.log`).

use serde_json::{Map, Value};

use recall_core::follower::Follower;
use recall_core::services::seventeenlands::SeventeenLands;
use recall_core::services::seventeenlands::{RecordingSubmitter, to_python_json_string};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(log), Some(golden)) = (args.next(), args.next()) else {
        eprintln!("usage: golden <logfile> <golden.jsonl>");
        std::process::exit(2);
    };

    let mut f = Follower::with_sink(SeventeenLands::new(
        "00000000-0000-4000-8000-000000000000".into(),
        RecordingSubmitter::new(),
    ));
    f.parse_log(&log, false);

    let lines: Vec<String> = f
        .sink
        .wire
        .calls
        .iter()
        .map(|c| {
            let mut m = Map::new();
            m.insert("endpoint".into(), Value::String(c.endpoint.clone()));
            m.insert("use_gzip".into(), Value::Bool(c.use_gzip));
            m.insert("payload".into(), c.payload.clone());
            to_python_json_string(&Value::Object(m))
        })
        .collect();

    match std::fs::read_to_string(&golden) {
        Err(_) => {
            if let Some(dir) = std::path::Path::new(&golden).parent() {
                std::fs::create_dir_all(dir).expect("create golden dir");
            }
            std::fs::write(&golden, lines.join("\n") + "\n").expect("write golden");
            eprintln!("{golden}: baseline written ({} submissions)", lines.len());
        }
        Ok(existing) => {
            let expected: Vec<&str> = existing.lines().collect();
            if expected.len() != lines.len() {
                eprintln!(
                    "{golden}: MISMATCH - {} submissions now vs {} in baseline",
                    lines.len(),
                    expected.len()
                );
                std::process::exit(1);
            }
            for (i, (got, want)) in lines.iter().zip(&expected).enumerate() {
                if got != want {
                    // Endpoint names are safe to print; payload bytes are not (real data).
                    let ep = |l: &str| l.split('"').nth(3).unwrap_or("?").to_string();
                    eprintln!(
                        "{golden}: MISMATCH at submission [{i}] ({}): payload diverges from baseline (bytes withheld - contains account data; diff {golden} manually)",
                        ep(got)
                    );
                    std::process::exit(1);
                }
            }
            eprintln!("{golden}: OK ({} submissions match)", lines.len());
        }
    }
}
