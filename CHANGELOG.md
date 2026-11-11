# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Restarts no longer re-upload the log. Recall checkpoints how far it has got
  (`<data_dir>/recall/state.json`) and on the next start rebuilds its state from the already
  processed part without sending anything, then carries on live. Applies to the CLI's follow mode
  (including the `Player-prev.log` catch-up) and the desktop app; `recall <LOG>` still processes the
  whole file. An upload 17Lands did not accept is offered again on the next start. The archive no
  longer gains duplicate lines on restart.

### Changed

- Uploads reuse one HTTP connection instead of opening a new one (TCP + TLS handshake) per event,
  and a stalled connection now times out and is retried instead of hanging the follower.
- The desktop app no longer polls: the window status updates when something changes, instead of
  re-reading `config.toml` every 2 seconds.
- Desktop app: rebuilt as a native app on GPUI, replacing the Tauri webview. The window is the log
  with one status line under it (last upload and upload count), Settings is its own window (⌘,), and
  both follow the system light/dark theme. The menu-bar menu is "Open Recall", "Settings…" and "Quit
  Recall"; its status line is gone. Same bundle identifier, config and checkpoint, so it upgrades in
  place. macOS only.
- Release binaries are built with LTO and stripped (the CLI is about a third smaller).
- The desktop app is signed with a Developer ID and notarized by Apple, so it opens without the
  Gatekeeper workaround.
- Releases are gated on the test suite and on the tag matching the manifest versions.

### Fixed

- Desktop: stopping and restarting the follower (saving a token, changing the log path) no longer
  drops the parser's state, which left events without a player id and lost a game in progress.
- Desktop: Quit, saving a token and changing the log path no longer hang while an upload is waiting
  to be retried (up to ten minutes when 17Lands is down or rate-limiting). The interrupted event is
  uploaded on the next start.
- `config.toml`, which holds the token, is now written readable by its owner only (0600) on macOS
  and Linux; an existing file is tightened the next time it is saved.
- Debug output no longer includes raw Arena log entries, which could show account details in the
  desktop log window.
- The declared minimum Rust version is now 1.88, which the code has needed since it adopted
  let-chains (it said 1.85).

## [0.3.1] - 2026-10-25

### Fixed

- Upload failures are no longer silent. A 4xx from 17Lands is logged as a warning (with the server's
  reason) instead of a debug line indistinguishable from success.
- Rate limiting no longer drops events: a 429, or the 403 17Lands' load-balancer firewall returns to
  a throttled IP, is retried with the same backoff as a 5xx, with a warning while it lasts.
- The desktop status counts only accepted uploads and shows failed ones separately.

## [0.3.0] - 2026-08-20

### Added

- Per-service configuration: `[services.<name>]` sections in `config.toml`
  (`[services.seventeenlands]` with `token` and optional `host`); both the CLI and the desktop app
  build their service set from it.
- The `archive` service: a local JSONL record of every parsed event, one `<dir>/<YYYY-MM-DD>.jsonl`
  file per day, enabled by adding `[services.archive]` (`dir`). Never affects the 17Lands upload.

### Changed

- Config layout auto-migrates: a flat top-level `token` moves into `services.seventeenlands.token`
  on first load.
- Internals reorganized around a neutral event stream: the parser emits typed `GameEvent`s and
  services (17Lands today) translate them. Wire output is unchanged.
- Relicensed under the MIT License (was GPL-3.0-only).

### Removed

- `--tee <URL>`. Secondary outputs are now services configured in `config.toml` rather than flags.
- The `--log-file`/`-l`, `--host`, `--token` and `--once` flags. The CLI is now `recall [LOG] [-v]`;
  the host comes from `services.seventeenlands.host`.
- The `~/.mtga_follower.ini` import and the interactive stdin token prompt. With no token configured
  the CLI prints the config path and exits.
- `NOTICE` and `ROADMAP.md`.
- The oracle parity harness (`tools/oracle/run_oracle.sh`, the `oracle_diff` example, and the
  `just oracle*`/`parity` recipes). It ran the brew-installed `seventeenlands`, which has been this
  Rust client itself since 0.1.1 - so it was diffing the client against itself. Parity is now
  maintained via the fixture tests and by diffing upstream Python source changes. The local mock
  server moved to `tools/mock_server.py` (still used for desktop dev).

## [0.2.0] - 2026-07-14

### Changed

- Renamed the product to **Recall** (after Ancestral Recall). The binary is now `recall` (was
  `seventeenlands`), the crates are `recall-core` / `recall` / `recall-desktop`, the desktop app is
  "Recall", the Homebrew formula/cask are `recall` / `recall-desktop`, and the repository is
  `fredoliveira/recall`. The token now lives at `<config_dir>/recall/config.toml` (tokens saved
  under the old `17l/` dir are not migrated - re-enter yours once). The wire protocol is unchanged:
  uploads still go to 17Lands, identified as the upstream Python client.

## [0.1.3] - 2026-07-13

### Added

- Desktop menu-bar app (Tauri v2) on the same parser as the CLI: tray status, a log window, token
  and log-path settings. Published as a `.dmg` with a Homebrew cask.

### Changed

- The repository is a Cargo workspace (`core`, `cli`, `desktop`), and the desktop bundle is built by
  its own release workflow.

## [0.1.2] - 2026-07-13

### Added

- `--tee <URL>`: also forward every parsed event to a local HTTP sink (e.g. a live draft companion
  app). Blobs POST as JSON to `<URL>/<event>`; fire-and-forget, so a missing or slow sink never
  affects the 17Lands upload.

## [0.1.1] - 2026-06-27

### Changed

- The installed executable is now named `seventeenlands` (was `seventeenlands-rust`), matching the
  official Python client it replaces. The crate, repository, and release artifacts keep the
  `seventeenlands-rust` name.

## [0.1.0] - 2026-06-26

Initial release. A drop-in, wire-compatible Rust port of the official 17Lands MTG Arena log client
([`mtga-log-client`](https://github.com/rconroy293/mtga-log-client)), targeting upstream
`CLIENT_VERSION = "0.1.44.p"`.

### Added

- Tails Arena's `Player.log`, parses gameplay events, and uploads them to the 17Lands REST API with
  byte-for-byte payload parity against the Python client.
- CLI flags: `--log-file`, `--host`, `--token`, `--once`.
- Token resolution chain: flag → platform config dir → legacy `~/.mtga_follower.ini` (migrated on
  first run) → interactive prompt.
- Oracle parity tooling (`tools/oracle/`, `examples/oracle_diff.rs`) and an offline `replay`
  example.

[Unreleased]: https://github.com/fredoliveira/recall/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/fredoliveira/recall/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/fredoliveira/recall/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/fredoliveira/recall/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/fredoliveira/recall/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/fredoliveira/recall/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/fredoliveira/recall/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/fredoliveira/recall/releases/tag/v0.1.0
