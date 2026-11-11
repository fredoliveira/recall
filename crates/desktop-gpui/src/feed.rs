//! What the windows show, and how it gets to them.
//!
//! The follower runs on its own thread and reports everything through the `log` crate and
//! the upload observer. Both push [`Signal`]s into one channel; a task on the main thread
//! (see `main.rs`) drains it into the [`Feed`] entity, and the windows observe that. Nothing
//! polls, and the feed outlives the windows, so reopening one shows what happened meanwhile.

use std::collections::VecDeque;

use gpui_kit::SharedString;
use log::{Level, LevelFilter, Log, Metadata, Record};
use smol::channel::Sender;

use crate::state::Status;

/// Lines kept for the log window.
const CAPACITY: usize = 2000;

pub enum Signal {
    /// Something the status shows changed (follower started or stopped, an upload outcome).
    Status,
    Log(LogLine),
    /// A menu-bar menu item was chosen; the string is its id.
    Menu(String),
}

#[derive(Clone)]
pub struct LogLine {
    pub ts: SharedString,
    pub level: Level,
    pub msg: SharedString,
    /// Background-sync chatter (`follower::CHATTER`), hidden unless asked for.
    pub chatter: bool,
}

#[derive(Default)]
pub struct Feed {
    pub lines: VecDeque<LogLine>,
    pub status: Status,
}

impl Feed {
    pub fn push(&mut self, line: LogLine) {
        if self.lines.len() >= CAPACITY {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// The global `log` sink. Installed before the app exists; lines queue in the channel until
/// the main thread starts draining it.
struct Logger(Sender<Signal>);

pub fn install_logger(tx: Sender<Signal>) {
    if log::set_boxed_logger(Box::new(Logger(tx))).is_ok() {
        // Debug: the 17Lands transport logs an accepted upload ("<endpoint> -> 200") at debug.
        log::set_max_level(LevelFilter::Debug);
    }
}

impl Log for Logger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        let target = record.target();
        // Ours only: third-party crates (the HTTP client) are noisy at debug.
        if !target.starts_with("recall") {
            return;
        }
        let line = LogLine {
            ts: chrono::Local::now().format("%H:%M:%S").to_string().into(),
            level: record.level(),
            msg: record.args().to_string().into(),
            chatter: target == recall_core::follower::CHATTER,
        };
        // The terminal gets the same feed when run from one.
        eprintln!("{} {:<5} {}", line.ts, line.level, line.msg);
        let _ = self.0.try_send(Signal::Log(line));
    }

    fn flush(&self) {}
}
