// SPDX-License-Identifier: MIT
//
// Recall - an MTG Arena log parser.
// Copyright (C) 2026 Fred Oliveira <hi@helloform.com>
//
// Licensed under the MIT License; see the LICENSE file at the repository root.

//! Recall - an MTG Arena log parser (library crate).
//!
//! One follower tails `Player.log` and turns it into a neutral [`events::GameEvent`]
//! stream; pluggable [`events::Service`]s turn those events into uploads or records
//! (17Lands, a local archive). The `recall` CLI and the desktop app are thin wrappers
//! over this crate; the modules are public so the integration tests in `tests/`
//! (fixture parity, HTTP) can drive them directly.

pub mod checkpoint;
pub mod config;
pub mod events;
pub mod follower;
pub mod paths;
pub mod retry;
pub mod services;
pub mod time_parse;
