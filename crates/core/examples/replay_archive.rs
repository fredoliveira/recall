// SPDX-License-Identifier: MIT

//! Summarize a JSONL archive written by `services::archive`: per-event counts and the
//! date range. Prints no payload values (archives contain account data).
//!
//! `cargo run -p recall-core --example replay_archive -- <file.jsonl>`

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: replay_archive <file.jsonl>");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let (mut first, mut last, mut bad) = (None::<String>, None::<String>, 0usize);
    for line in BufReader::new(std::fs::File::open(&path)?).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line?) else {
            bad += 1;
            continue;
        };
        let name = v["event"].as_str().unwrap_or("?").to_string();
        *counts.entry(name).or_default() += 1;
        if let Some(t) = v["time"].as_str() {
            first = Some(first.map_or(t.into(), |f: String| f.min(t.into())));
            last = Some(last.map_or(t.into(), |l: String| l.max(t.into())));
        }
    }
    for (name, n) in &counts {
        println!("{n:>7}  {name}");
    }
    println!("{:>7}  total", counts.values().sum::<usize>());
    if let (Some(f), Some(l)) = (first, last) {
        println!("range: {f} .. {l}");
    }
    if bad > 0 {
        println!("{bad} unparseable line(s)");
    }
    Ok(())
}
