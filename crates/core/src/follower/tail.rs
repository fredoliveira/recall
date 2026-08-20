// SPDX-License-Identifier: MIT

//! Tailing and line accumulation: the follow loop with rotation detection, cooperative
//! cancellation and resume-from-offset, and the entry-boundary logic that turns raw lines
//! into complete log entries (port of `Follower.parse_log` / `__append_line` /
//! `__handle_complete_log_entry`).

use super::*;

impl<S: Service> Follower<S> {
    /// Tail (or read once) a log file, dispatching complete entries (port of `parse_log`).
    pub fn parse_log(&mut self, filename: &str, follow: bool) {
        // The plain entry point never cancels: a flag that is always `false` makes
        // `parse_log_cancellable` behave exactly like the original loop. Existing callers
        // and tests are unaffected.
        let never = Arc::new(AtomicBool::new(false));
        self.parse_log_cancellable(filename, follow, &never);
    }

    /// Like [`parse_log`](Self::parse_log) but honours a cooperative cancellation flag,
    /// returning within one [`SLEEP_TIME`] tick once `cancel` is set. This is additive
    /// control flow only: it changes nothing about which payloads are built or sent, so the
    /// wire contract (and parity tests) are unaffected. Used by the desktop app's start/stop.
    pub fn parse_log_cancellable(
        &mut self,
        filename: &str,
        follow: bool,
        cancel: &Arc<AtomicBool>,
    ) {
        // Delegate with `start_offset = 0` (read from the beginning) and no position sink:
        // this reproduces the original loop byte-for-byte for every existing caller (CLI,
        // parity tests). See [`parse_log_cancellable_from`].
        self.parse_log_cancellable_from(filename, follow, cancel, 0, None);
    }

    /// Like [`parse_log_cancellable`](Self::parse_log_cancellable) but supports *resuming* a
    /// tail from a remembered byte offset, and continuously reports the current read position.
    /// This is additive control flow only - it neither builds nor sends a different payload,
    /// so the wire contract (and parity tests) are unchanged; with `start_offset = 0` and
    /// `position = None` it is identical to the original loop.
    ///
    /// - `start_offset`: on the *very first* file open this seeks to that byte instead of byte
    ///   0, skipping the from-zero reprocessing for that first pass. Used by the desktop app to
    ///   avoid re-emitting old lines and re-POSTing already-processed events on stop→start.
    ///   It applies only to the initial open: the in-loop "start from beginning" heuristics
    ///   (file shrank, or modified far more recently than the last read) still restart the tail
    ///   from byte 0 as before, so live-tailing is unaffected.
    /// - `position`: when `Some`, the current byte read-position is published into this atomic
    ///   as lines are consumed, so a caller can remember where the tail left off at stop time.
    pub fn parse_log_cancellable_from(
        &mut self,
        filename: &str,
        follow: bool,
        cancel: &Arc<AtomicBool>,
        start_offset: u64,
        position: Option<&Arc<AtomicU64>>,
    ) {
        // The resume offset applies to the initial open only; every subsequent "start from
        // beginning" pass reads from byte 0 like the original loop.
        let mut pending_offset = start_offset;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            self.reinitialize();
            let mut last_read_time = SystemTime::now();
            let mut last_file_size: u64 = 0;
            let resume_offset = std::mem::replace(&mut pending_offset, 0);

            match std::fs::File::open(filename) {
                Ok(file) => {
                    let mut reader = BufReader::new(file);
                    // Byte position of the read cursor, mirrored into `position` as we go.
                    let mut pos: u64 = 0;
                    if resume_offset > 0 {
                        match reader.seek(SeekFrom::Start(resume_offset)) {
                            Ok(p) => {
                                pos = p;
                                // Seed the shrink heuristic so a file that is still at least as
                                // large as our offset is not mistaken for a truncation.
                                last_file_size = p;
                            }
                            Err(e) => {
                                log::error!("Error seeking log to resume offset: {e}");
                            }
                        }
                    }
                    if let Some(position) = position {
                        position.store(pos, Ordering::Relaxed);
                    }
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let mut raw = Vec::new();
                        let read = read_until_newline(&mut reader, &mut raw);
                        let file_size = std::fs::metadata(filename).map(|m| m.len()).unwrap_or(0);

                        match read {
                            Ok(0) => {
                                // EOF.
                                self.handle_complete_log_entry();
                                let last_modified = std::fs::metadata(filename)
                                    .and_then(|m| m.modified())
                                    .unwrap_or(SystemTime::UNIX_EPOCH);

                                if file_size < last_file_size {
                                    log::info!(
                                        "Starting from beginning of file as file is smaller than before (previous = {last_file_size}; current = {file_size})"
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
                                last_file_size = file_size;
                                pos += n as u64;
                                if let Some(position) = position {
                                    position.store(pos, Ordering::Relaxed);
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

            if !follow {
                log::info!("Done processing file.");
                break;
            }
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
            log::warn!("Detailed logs are disabled in MTGA.");
        } else if line.starts_with("DETAILED LOGS: ENABLED") {
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
        self.current_debug_blob = full_log.clone();

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

/// Read one line including its trailing `\n` (mirrors Python `readline` with
/// `errors="replace"` handled by the lossy decode at the call site). Returns bytes read.
fn read_until_newline<R: Read>(
    reader: &mut BufReader<R>,
    buf: &mut Vec<u8>,
) -> std::io::Result<usize> {
    use std::io::BufRead;
    reader.read_until(b'\n', buf)
}
