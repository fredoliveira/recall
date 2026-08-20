// SPDX-License-Identifier: MIT

//! Replay a Player.log through the Follower with a recording submitter, printing a summary
//! of what would be submitted. Dev tooling for validating the parser on real logs without
//! posting anywhere. Usage: `cargo run --example replay -- <logfile>`
//!
//! Prints only endpoint counts and top-level key names (never payload values) to avoid
//! echoing account ids / screen names from a real log.

use std::collections::BTreeMap;

use recall_core::follower::Follower;
use recall_core::services::seventeenlands::RecordingSubmitter;
use recall_core::services::seventeenlands::SeventeenLands;

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay <logfile>");
    let mut f = Follower::with_sink(SeventeenLands::new(
        "00000000-0000-4000-8000-000000000000".into(),
        RecordingSubmitter::new(),
    ));
    f.parse_log(&path, false);

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in &f.sink.wire.calls {
        *counts.entry(c.endpoint.clone()).or_default() += 1;
    }
    eprintln!("total submissions: {}", f.sink.wire.calls.len());
    for (ep, n) in &counts {
        eprintln!("  {ep}: {n}");
    }

    if let Some(g) = f
        .sink
        .wire
        .calls
        .iter()
        .find(|c| c.endpoint.ends_with("add_game"))
        && let Some(obj) = g.payload.as_object()
    {
        eprintln!(
            "add_game top-level keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        if let Some(hist) = obj.get("history").and_then(|h| h.as_object()) {
            eprintln!(
                "add_game.history keys: {:?}; events={}",
                hist.keys().collect::<Vec<_>>(),
                hist.get("events")
                    .and_then(|e| e.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0),
            );
        }
    }
}
