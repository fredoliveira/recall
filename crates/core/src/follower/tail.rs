// SPDX-License-Identifier: MIT

//! Tailing and line accumulation: the follow loop with rotation detection, cooperative
//! cancellation and resume-from-offset, and the entry-boundary logic that turns raw lines
//! into complete log entries (port of `Follower.parse_log` / `__append_line` /
//! `__handle_complete_log_entry`).

use super::*;

impl<S: Service> Follower<S> {
    /// Tail (or read once) a log file, dispatching complete entries (port of `parse_log`).
    pub fn parse_log(&mut self, filename: &str, follow: bool) {
        // The plain entry point never cancels and keeps no checkpoint: it is the original
        // loop, for the parity tests and one-shot parses.
        let never = Arc::new(AtomicBool::new(false));
        self.parse_log_cancellable(filename, follow, &never);
    }

    /// Like [`parse_log`](Self::parse_log) but honours a cooperative cancellation flag,
    /// returning within one [`SLEEP_TIME`] tick once `cancel` is set. Control flow only:
    /// nothing about which payloads are built or sent changes.
    pub fn parse_log_cancellable(
        &mut self,
        filename: &str,
        follow: bool,
        cancel: &Arc<AtomicBool>,
    ) {
        self.parse_log_resumable(filename, follow, cancel, None);
    }

    /// Like [`parse_log_cancellable`](Self::parse_log_cancellable), and with `checkpoints`
    /// it also remembers how far the services have been fed, so the next run does not hand
    /// them the same events again (see [`crate::checkpoint`]).
    ///
    /// Resuming is a **muted replay**, not a seek: when a checkpoint matches the file, the
    /// pass still reads from byte 0 and runs every handler, so the state is rebuilt exactly
    /// as the first run built it, but events are dropped instead of emitted until the read
    /// position reaches the checkpoint. From there on the loop is the ordinary live one.
    /// Payloads are built by the same code from the same state, so the wire contract is
    /// untouched; the only difference is which events are sent at all.
    ///
    /// The checkpoint moves forward only past events every service took, at the end of
    /// the line (or end-of-file flush) that emitted them. Delivery is therefore
    /// at-least-once: an interrupted or failed upload is offered again on the next run.
    pub fn parse_log_resumable(
        &mut self,
        filename: &str,
        follow: bool,
        cancel: &Arc<AtomicBool>,
        mut checkpoints: Option<&mut Checkpoints>,
    ) {
        let log_path = Path::new(filename);
        // Set when a replay ran out of file before its checkpoint: the next pass starts
        // from byte 0, live.
        let mut skip_resume = false;

        'pass: loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            self.reinitialize();
            self.muted = false;
            self.delivered_since_checkpoint = false;
            self.checkpoint_blocked = false;
            let mut last_read_time = SystemTime::now();

            // Looked up on every pass, not just the first: a pass restarted over the same
            // file resumes too, and a rotated file simply matches no checkpoint.
            let resume = if std::mem::take(&mut skip_resume) {
                None
            } else {
                checkpoints
                    .as_deref()
                    .and_then(|c| c.resume_point(log_path))
            };
            // Handlers log as they go; during the replay that would narrate old news.
            let mut quiet = None;
            if let Some(r) = resume {
                log::info!(
                    target: CHATTER,
                    "Resuming {filename}: skipping {} bytes already processed",
                    r.offset
                );
                self.muted = true;
                quiet = Some(Quiet::begin());
            }

            match std::fs::File::open(filename) {
                Ok(file) => {
                    let mut reader = BufReader::new(file);
                    // Byte position of the read cursor.
                    let mut pos: u64 = 0;
                    // Whether `pos` is just past a newline. Arena can be caught mid-write;
                    // a checkpoint is only ever taken on a whole line, so a replay (which
                    // sees that line whole) stops at the same place.
                    let mut at_line_end = true;
                    let mut raw = Vec::new();
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            self.muted = false;
                            return;
                        }
                        raw.clear();

                        match reader.read_until(b'\n', &mut raw) {
                            Ok(0) if self.muted => {
                                // The file ended before the checkpoint it matched a moment
                                // ago: it was replaced under us. Start over without one.
                                self.muted = false;
                                drop(quiet.take());
                                log::info!(
                                    "Starting from beginning of file as it changed while resuming"
                                );
                                skip_resume = true;
                                continue 'pass;
                            }
                            Ok(0) => {
                                // EOF.
                                self.handle_complete_log_entry();
                                self.save_checkpoint(
                                    checkpoints.as_deref_mut(),
                                    log_path,
                                    pos,
                                    true,
                                    at_line_end,
                                );

                                let metadata = std::fs::metadata(filename);
                                let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                                let last_modified = metadata
                                    .and_then(|m| m.modified())
                                    .unwrap_or(SystemTime::UNIX_EPOCH);

                                if file_size < pos {
                                    log::info!(
                                        "Starting from beginning of file as file is smaller than before (previous = {pos}; current = {file_size})"
                                    );
                                    break;
                                } else if last_modified
                                    > last_read_time
                                        + Duration::from_secs(FILE_UPDATED_FORCE_REFRESH_SECONDS)
                                {
                                    log::info!(
                                        "Starting from beginning of file as file has been updated much more recently than the last read"
                                    );
                                    break;
                                } else if follow {
                                    std::thread::sleep(SLEEP_TIME);
                                } else {
                                    break;
                                }
                            }
                            Ok(n) => {
                                let line = String::from_utf8_lossy(&raw);
                                self.append_line(&line);
                                last_read_time = SystemTime::now();
                                pos += n as u64;
                                at_line_end = raw.ends_with(b"\n");

                                match resume {
                                    Some(r) if self.muted => {
                                        if pos >= r.offset {
                                            // The first run flushed the entry buffer here
                                            // (it had hit end of file); do the same before
                                            // going live.
                                            if r.flushed && pos == r.offset {
                                                self.handle_complete_log_entry();
                                            }
                                            self.muted = false;
                                            drop(quiet.take());
                                            self.replay_finished();
                                        }
                                    }
                                    _ => self.save_checkpoint(
                                        checkpoints.as_deref_mut(),
                                        log_path,
                                        pos,
                                        false,
                                        at_line_end,
                                    ),
                                }
                            }
                            Err(e) => {
                                log::error!("Error parsing log: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::thread::sleep(SLEEP_TIME);
                }
                Err(e) => {
                    log::error!("Error opening log: {e}");
                }
            }
            // A pass can only end muted through one of the early exits above.
            self.muted = false;

            if !follow {
                log::info!("Done processing file.");
                break;
            }
        }
    }

    /// Move the checkpoint to `pos` if something was delivered since it was last saved and
    /// nothing has been refused in this pass. Replaying log that emitted nothing is cheap,
    /// so the state file is only rewritten when there is an upload to avoid repeating.
    fn save_checkpoint(
        &mut self,
        checkpoints: Option<&mut Checkpoints>,
        log_path: &Path,
        pos: u64,
        flushed: bool,
        at_line_end: bool,
    ) {
        let Some(checkpoints) = checkpoints else {
            return;
        };
        if self.delivered_since_checkpoint && !self.checkpoint_blocked && at_line_end {
            checkpoints.record(log_path, pos, flushed);
            self.delivered_since_checkpoint = false;
        }
    }

    /// The muted replay reached its checkpoint; logging is back on.
    fn replay_finished(&mut self) {
        // The replay swallowed this warning along with everything else, and it is the one
        // line a user must not miss.
        if self.detailed_logs_disabled {
            log::warn!("Detailed logs are disabled in MTGA.");
        }
    }

    /// Process a complete in-memory log (test seam): reinitialize, feed each
    /// line, then flush the final entry. Equivalent to `parse_log(.., follow=false)`
    /// without file I/O or the tail loop.
    pub fn process_str(&mut self, data: &str) {
        self.reinitialize();
        for line in data.split_inclusive('\n') {
            self.append_line(line);
        }
        self.handle_complete_log_entry();
    }

    /// Port of `__append_line` (`recent_lines` dropped).
    pub(super) fn append_line(&mut self, line: &str) {
        // __check_detailed_logs: keep the warning, drop the GUI dialog.
        if line.starts_with("DETAILED LOGS: DISABLED") {
            self.detailed_logs_disabled = true;
            log::warn!("Detailed logs are disabled in MTGA.");
        } else if line.starts_with("DETAILED LOGS: ENABLED") {
            self.detailed_logs_disabled = false;
            log::info!(target: CHATTER, "Detailed logs enabled in MTGA.");
        }

        self.maybe_handle_account_info(line);

        if let Some(c) = re_timestamp().captures(line) {
            self.last_raw_time = c[1].to_string();
            if let Ok(t) = extract_time(&self.last_raw_time) {
                self.cur_log_time = t;
            }
        }

        if let Some(m) = re_log_start_untimed().find(line) {
            self.handle_complete_log_entry();

            if let Some(tc) = re_log_start_timed().captures(line) {
                self.last_raw_time = tc.get(2).map(|g| g.as_str()).unwrap_or("").to_string();
                if let Ok(t) = extract_time(&self.last_raw_time) {
                    self.cur_log_time = t;
                }
                let end = tc.get(0).unwrap().end();
                self.buffer.push(line[end..].to_string());
            } else {
                self.buffer.push(line[m.end()..].to_string());
            }
        } else {
            self.buffer.push(line.to_string());
        }
    }

    /// Port of `__handle_complete_log_entry` (dedup; `cur_log_time` reset stays omitted).
    pub(super) fn handle_complete_log_entry(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        // cur_log_time is never None in this port (the reset is commented out in Python).

        let full_log = self.buffer.concat();

        if full_log != self.last_blob {
            self.handle_blob(&full_log);
            self.last_blob = full_log;
        } else {
            // Dedup hits are pure noise on the console (e.g. repeated "AK Message: Audio
            // thread resumed" lines); keep them at debug so `RUST_LOG=debug` can still show
            // them. Console-only - no effect on the wire (upstream logs this at info).
            log::debug!("Skipping repeated complete log entry: {full_log}");
        }

        self.buffer.clear();
    }
}

/// Silences the `log` macros for the duration of a muted replay, restoring the previous
/// level on drop. Global, which is fine here: the replay runs on the one follower thread
/// and nothing else in the process has anything to say meanwhile.
struct Quiet(log::LevelFilter);

impl Quiet {
    fn begin() -> Self {
        let previous = log::max_level();
        log::set_max_level(log::LevelFilter::Off);
        Self(previous)
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        log::set_max_level(self.0);
    }
}
