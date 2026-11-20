// SPDX-License-Identifier: MIT

//! Print what a live view is told while a log is parsed: one line per event, and one per
//! change to the game in progress. Uploads nothing.
//!
//! Usage: `cargo run -p recall-core --example live -- <LOG>`

use recall_core::events::{GameEvent, Service};
use recall_core::follower::Follower;
use recall_core::live::{Live, View};

struct Nowhere;

impl Service for Nowhere {
    fn handle(&mut self, _: &GameEvent) -> bool {
        true
    }
}

struct Printer;

impl View for Printer {
    fn handle(&mut self, update: Live<'_>) {
        match update {
            Live::Event { event, .. } => println!("event {}", event.name()),
            Live::Game(None) => println!("game  over"),
            Live::Game(Some(game)) => {
                let left: usize = game.remaining().iter().map(|c| c.left).sum();
                println!(
                    "game  turn {:>2}  library {:>2}  left {:>2}  hand {}  seen {}  opponent {}",
                    game.turn,
                    game.library.map_or("?".into(), |n| n.to_string()),
                    left,
                    game.hand.len(),
                    game.seen.len(),
                    game.opponent_seen.len(),
                );
            }
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: live <LOG>");
    Follower::with_sink(Nowhere)
        .with_view(Printer)
        .parse_log(&path, false);
}
