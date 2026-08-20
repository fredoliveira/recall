// SPDX-License-Identifier: MIT

//! Service adapters: each turns the follower's [`GameEvent`](crate::events::GameEvent)
//! stream into one destination's format, owning that destination's credentials, payload
//! shape, routing, and retry policy. Compose several with [`fanout::Fanout`].
//!
//! - [`seventeenlands`] - the default, primary service (blocking, retrying; the parity surface).
//! - [`archive`] - local JSONL record in the neutral JSON view; the minimal secondary.

pub mod archive;
pub mod fanout;
pub mod seventeenlands;

use crate::config::Config;
use crate::events::Service;
use seventeenlands::{SeventeenLands, Submitter};

/// Build the service set both binaries run: 17Lands (`token`, `host`, over `wire`) first,
/// then an [`archive::Archive`] when `[services.archive]` is configured. Returns the
/// 17Lands service alone when it is the only one, so the primary path has no fan-out layer.
pub fn from_config(
    cfg: &Config,
    token: String,
    wire: impl Submitter + 'static,
) -> Box<dyn Service> {
    let primary = Box::new(SeventeenLands::new(token, wire));
    match cfg.services.archive.as_ref().and_then(|a| a.resolved_dir()) {
        Some(dir) => Box::new(fanout::Fanout::new(vec![
            primary,
            Box::new(archive::Archive::new(dir)),
        ])),
        None => primary,
    }
}
