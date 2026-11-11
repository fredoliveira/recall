# Recall - an MTG Arena log parser

[![CI](https://github.com/fredoliveira/recall/actions/workflows/ci.yml/badge.svg)](https://github.com/fredoliveira/recall/actions/workflows/ci.yml)

Recall is an MTG Arena log parser. It tails Arena's `Player.log`, parses drafts, games, decks, ranks
and more into a neutral event stream, then hands that stream to services: 17Lands by default, and
optionally a local JSONL archive of your own data. The original motivation for Recall was a
single-binary distribution with no Python dependencies that is easier to build on top of.

The 17Lands service is inspired by the official
[17Lands Python client](https://github.com/rconroy293/mtga-log-client) and is a drop-in replacement
for it: what Recall sends to 17Lands is byte-for-byte identical to the original.

## Install

```sh
# CLI via Homebrew (macOS/Linux):
brew install fredoliveira/tap/recall

# Desktop menu-bar app via Homebrew (macOS):
brew install --cask fredoliveira/tap/recall-desktop

# CLI from source (builds and installs the binary onto your PATH):
cargo install --git https://github.com/fredoliveira/recall recall
```

On Windows, download `recall-x86_64-pc-windows-msvc.zip` from the
[latest release](https://github.com/fredoliveira/recall/releases/latest), unzip it, and run
`recall.exe` from a terminal (or build from source with the `cargo install` line above).

## Running

```sh
# Assuming a normal installation, run this in your terminal
recall
```

**Detailed Logs** must be enabled in MTGA (gear → Account → "Detailed Logs") for game data to be
captured.

## Configuration

`<config_dir>/recall/config.toml` (`~/.config/recall/config.toml` on Linux,
`~/Library/Application Support/recall/config.toml` on macOS, `%APPDATA%\recall\config.toml` on
Windows). One section per service; a present section enables it. Create it with your 17Lands token
(from [17lands.com/account](https://www.17lands.com/account)); without one, `recall` prints the path
and exits.

```toml
[services.seventeenlands]
token = "00000000-0000-4000-8000-000000000000"
# host = "https://api.17lands.com"      # optional override

[services.archive]                      # present = enabled
# dir = "/path/to/archive"              # optional; default below
```

### Restarts

Recall remembers how far into each log it has got (`state.json`, next to the archive directory
below), so restarting it does not upload the same events again. To force a full re-upload of a log,
pass it explicitly - `recall path/to/Player.log` always processes the whole file - or delete
`state.json`.

### Keeping your own data: the archive

Add an empty `[services.archive]` section and every parsed event is also written locally,
service-neutral, alongside the 17Lands upload. Files land in
`~/Library/Application Support/recall/archive/` on macOS and `~/.local/share/recall/archive/` on
Linux, and `%APPDATA%\recall\archive\` on Windows (override with `dir`): one `<YYYY-MM-DD>.jsonl`
per day, dated by the log timestamp, one JSON object per line -
`{"v": 1, "event": "<name>", ...event fields}`. Summarize a file with
`cargo run -p recall-core --example replay_archive -- <file.jsonl>`.

## Repository layout

Two artifacts from a shared core. The CLI and core are one Cargo workspace; the desktop app is its
own (it pins a newer toolchain):

| Crate            | Path             | Artifact                                                                                         |
| ---------------- | ---------------- | ------------------------------------------------------------------------------------------------ |
| `recall-core`    | `crates/core`    | shared library: the parser (`follower`), the event model (`events`), the services (`services/…`) |
| `recall`         | `crates/cli`     | the `recall` CLI                                                                                 |
| `recall-desktop` | `crates/desktop` | macOS menu-bar app on GPUI (see its [README](crates/desktop/README.md))                          |

Bare `cargo build`/`test` build the CLI + core; the desktop app builds from `crates/desktop` or with
the `just desktop-*` recipes.

## Development

Common tasks live in the [`justfile`](justfile) (`brew install just`):

| Command              | What it does                                                                          |
| -------------------- | ------------------------------------------------------------------------------------- |
| `just run [ARGS]`    | Build and run the CLI against the auto-detected `Player.log`                          |
| `just desktop-dev`   | Run the desktop app pointed at a local mock API                                       |
| `just desktop-run`   | Build and run the desktop app (release; uploads to the live host)                     |
| `just desktop-build` | Build the signed desktop bundle (`.app` + `.dmg`)                                     |
| `just test`          | Run the test suite                                                                    |
| `just lint`          | Pre-commit gate: format check + clippy + tests (mirrors CI)                           |
| `just replay LOG`    | Replay a log offline, printing endpoint counts without uploading                      |
| `just golden`        | Byte-compare parser output for the real logs in `local/logs/` against saved baselines |

Wire compatibility with 17Lands is enforced by fixture tests (`cargo test`) and the golden
baselines; see [`CLAUDE.md`](CLAUDE.md) for the rules. Adding a service means implementing one trait
(`events::Service`) - the archive service is the worked example.

## Credits & license

Recall is heavily inspired by the great work of the 17lands team and the original
[`rconroy293/mtga-log-client`](https://github.com/rconroy293/mtga-log-client) client.

Recall is released under the [MIT License](LICENSE).

Unofficial Fan Content permitted under Wizards of the Coast's Fan Content Policy; not
approved/endorsed by Wizards. Portions © Wizards of the Coast LLC.
