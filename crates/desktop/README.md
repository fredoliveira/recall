# Recall desktop

A macOS menu-bar app on the Rust log follower core, built with [GPUI](https://gpui.rs): a log
window, a Settings window (⌘,) and a menu-bar icon. The follower runs on a background thread; the
app reads and writes the same `config.toml` as the CLI.

It is its own workspace (see the root `Cargo.toml`) because GPUI needs a newer toolchain and several
hundred crates the CLI never builds.

## Install

```sh
brew install --cask fredoliveira/tap/recall-desktop
```

The release workflow attaches a universal `.dmg` (Intel + Apple Silicon), signed with a Developer ID
and notarized by Apple. See [`packaging/homebrew/README.md`](../../packaging/homebrew/README.md).

## Develop

```sh
# Never POST to the live API during dev - point at the local mock server.
python3 ../../tools/mock_server.py 8732 /tmp/desktop-out.jsonl &
RECALL_HOST=http://127.0.0.1:8732 cargo +1.99.1 run
```

Or `just desktop-dev` from the repo root. The toolchain is named explicitly because a global
`RUSTUP_TOOLCHAIN` (mise sets one) overrides `rust-toolchain.toml`.

| Variable            | Effect                                                              |
| ------------------- | ------------------------------------------------------------------- |
| `RECALL_HOST`       | Upload host override. **Always set this to the mock in dev.**       |
| `RECALL_LOG`        | Follow this file instead of the auto-detected `Player.log`          |
| `RECALL_TRAY=0`     | No menu-bar icon: Recall runs as an ordinary Dock app               |
| `RECALL_APPEARANCE` | `light` or `dark` instead of following the system (for screenshots) |

## Layout

| File                | Role                                                                           |
| ------------------- | ------------------------------------------------------------------------------ |
| `src/main.rs`       | App setup: menus, keys, the menu-bar icon, the task that drains the feed       |
| `src/feed.rs`       | `Signal` channel, the `log` sink, and the `Feed` entity the windows watch      |
| `src/log_window.rs` | The main window: the log, and a status line under it                           |
| `src/settings.rs`   | The Settings window: token and log file                                        |
| `src/tray.rs`       | The menu-bar icon (`tray-icon`; GPUI has no status-item API)                   |
| `src/style.rs`      | Every size in the windows, in one place to tweak                               |
| `src/state.rs`      | `AppState`: follower thread + cancellation, config, shared upload status       |
| `src/observer.rs`   | `ObservingSubmitter` - records endpoint/time/count without altering POSTs      |
| `scripts/bundle.sh` | Builds and signs `dist/Recall.app` and the `.dmg` (the release workflow's too) |

Dependencies: `gpui-kit` 0.9.0, which pins its GPUI snapshot (`gpui-pre` 0.3.11).

## Bundle

```sh
scripts/bundle.sh --universal --dmg   # dist/Recall.app and dist/Recall-universal.dmg
```

Or `just desktop-build`. Signed with your Developer ID (hardened runtime) but not notarized: that is
the release workflow's job. The bundle has the installed app's identifier and, like it, uploads to
the live host: quit the installed Recall before opening it. The universal build needs
`rustup target add x86_64-apple-darwin` on the pinned toolchain.

## TODO

- UI tests (GPUI has a headless harness).
- Selecting and copying log text.
