// SPDX-License-Identifier: MIT
//
// Recall - an MTG Arena log parser.
// Copyright (C) 2026 Fred Oliveira <hi@helloform.com>
//
// Licensed under the MIT License; see the LICENSE file at the repository root.

//! Recall - an MTG Arena log parser.
//!
//! The thin CLI over `recall-core`: it resolves the config and token, sets up logging,
//! builds the configured services, and runs the processing loop.

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, ValueHint};

use recall_core::config::{self, Config};
use recall_core::events::Service;
use recall_core::follower::{self, Follower};
use recall_core::services::seventeenlands::ApiClient;
use recall_core::{paths, services};

/// Recall - an MTG Arena log parser.
///
/// Follows Arena's Player.log and feeds every event to the services configured in
/// `<config_dir>/recall/config.toml` (17Lands by default, plus an optional local archive).
/// Given a log file it parses that file once and exits instead.
#[derive(Parser, Debug)]
#[command(version, about, long_about)]
struct Args {
    /// Parse this log file once and exit, instead of following Arena's Player.log.
    #[arg(value_name = "LOG", value_hint = ValueHint::FilePath)]
    log_file: Option<PathBuf>,

    /// Detailed developer log format (date, milliseconds, level, module target).
    #[arg(short, long)]
    verbose: bool,
}

fn main() {
    let args = Args::parse();
    init_logging(args.verbose);

    // `Config::load` runs the flat-token migration; without a token the CLI explains
    // where it goes and exits.
    let cfg = Config::load();
    let token = config::resolve_token(&cfg);
    log::info!(
        target: follower::CHATTER,
        "Using token {}...{}",
        &token[..token.len().min(4)],
        &token[token.len().saturating_sub(4)..]
    );
    let host = cfg.seventeenlands_host();
    log::info!(target: follower::CHATTER, "Services: {}", cfg.service_names().join(", "));

    let sink = services::from_config(&cfg, token, ApiClient::new(host));
    let follower = Follower::with_sink(sink);
    match args.log_file {
        Some(path) => parse_once(path, follower),
        None => follow_arena(follower),
    }
}

/// stdout/stderr logging only - no rotating file handler. The console output is
/// purely cosmetic; it is **not** part of the wire contract (that lives in `services::seventeenlands`).
///
/// Default ("clean") format: `HH:MM:SS  message`, with the time dimmed and only WARN/ERROR
/// carrying a colored level tag - a calm status feed. `--verbose` switches to the detailed
/// developer layout that mirrors the Python `logging_utils` line
/// (`<date> <time>.<ms>,<level>,<target>,<message>`), useful for debugging / parity work.
///
/// Color is emitted unconditionally; env_logger's anstream-backed buffer strips the ANSI
/// codes automatically when the output is not a terminal or `NO_COLOR` is set.
fn init_logging(verbose: bool) {
    use env_logger::{Builder, Env};

    let mut builder = Builder::from_env(Env::default().default_filter_or("info"));

    if verbose {
        builder.format(|buf, record| {
            let now = chrono::Local::now();
            writeln!(
                buf,
                "{}.{:03},{},{},{}",
                now.format("%Y%m%d %H%M%S"),
                now.timestamp_subsec_millis(),
                record.level(),
                record.target(),
                record.args(),
            )
        });
    } else {
        use anstyle::{AnsiColor, Style};

        let dim = Style::new().dimmed();
        let yellow = Style::new().fg_color(Some(AnsiColor::Yellow.into())).bold();
        let red = Style::new().fg_color(Some(AnsiColor::Red.into())).bold();

        builder.format(move |buf, record| {
            let ts = chrono::Local::now().format("%H:%M:%S");
            match record.level() {
                log::Level::Warn => writeln!(
                    buf,
                    "{dim}{ts}{dim:#}  {yellow}WARN{yellow:#}  {yellow}{}{yellow:#}",
                    record.args()
                ),
                log::Level::Error => writeln!(
                    buf,
                    "{dim}{ts}{dim:#}  {red}ERROR{red:#}  {red}{}{red:#}",
                    record.args()
                ),
                // Background-sync chatter: dim the whole line so it recedes behind events.
                _ if record.target() == follower::CHATTER => {
                    writeln!(buf, "{dim}{ts}  {}{dim:#}", record.args())
                }
                // INFO/DEBUG/TRACE: dim time, message normal - keep the feed readable.
                _ => writeln!(buf, "{dim}{ts}{dim:#}  {}", record.args()),
            }
        });
    }

    builder.init();
}

/// Parse one log file and exit.
fn parse_once<S: Service>(path: PathBuf, mut follower: Follower<S>) {
    if !path.exists() {
        log::error!("{} does not exist", path.display());
        std::process::exit(1);
    }
    log::info!(target: follower::CHATTER, "Parsing {}", path.display());
    follower.parse_log(&path.to_string_lossy(), false);
    log::info!("Exiting");
}

/// Normal mode: catch up on the previous log once, then follow the current one.
fn follow_arena<S: Service>(mut follower: Follower<S>) {
    if let Some(prev) = paths::possible_previous_filepaths()
        .iter()
        .find(|f| f.exists())
    {
        log::info!(target: follower::CHATTER, "Parsing the previous log {} once", prev.display());
        follower.parse_log(&prev.to_string_lossy(), false);
    }

    let current: Vec<PathBuf> = paths::possible_current_filepaths()
        .into_iter()
        .filter(|f| f.exists())
        .collect();
    if current.is_empty() {
        log::warn!(
            "Found no Player.log to follow. Pass the path to it as an argument: `recall <LOG>`"
        );
    }
    for filename in &current {
        log::info!(target: follower::CHATTER, "Following along {}", filename.display());
        follower.parse_log(&filename.to_string_lossy(), true);
    }

    log::info!("Exiting");
}
