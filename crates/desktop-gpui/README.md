# Recall desktop on GPUI (spike)

The app in [`crates/desktop`](../desktop) rebuilt on [GPUI](https://gpui.rs) instead of Tauri: a log
window, a Settings window (⌘,) and a menu-bar icon, in front of the same `recall-core`. It is its
own workspace (see the root `Cargo.toml`) because GPUI needs a newer toolchain and several hundred
crates the CLI never builds.

## Run

```sh
# Never POST to the live API during dev - point at the local mock server.
python3 ../../tools/mock_server.py 8732 /tmp/desktop-out.jsonl &
RECALL_HOST=http://127.0.0.1:8732 cargo +1.99.1 run
```

Or `just desktop-gpui-dev` from the repo root. The toolchain is named explicitly because a global
`RUSTUP_TOOLCHAIN` (mise sets one) overrides `rust-toolchain.toml`.

| Variable            | Effect                                                              |
| ------------------- | ------------------------------------------------------------------- |
| `RECALL_HOST`       | Upload host override. **Always set this to the mock in dev.**       |
| `RECALL_LOG`        | Follow this file instead of the auto-detected `Player.log`          |
| `RECALL_TRAY=0`     | No menu-bar icon: Recall runs as an ordinary Dock app               |
| `RECALL_APPEARANCE` | `light` or `dark` instead of following the system (for screenshots) |

## Layout

| File                | Role                                                                      |
| ------------------- | ------------------------------------------------------------------------- |
| `src/main.rs`       | App setup: menus, keys, the menu-bar icon, the task that drains the feed  |
| `src/feed.rs`       | `Signal` channel, the `log` sink, and the `Feed` entity the windows watch |
| `src/log_window.rs` | The main window: status line, log, footer                                 |
| `src/settings.rs`   | The Settings window: token and log file                                   |
| `src/tray.rs`       | The menu-bar icon (`tray-icon`; GPUI has no status-item API)              |
| `src/style.rs`      | Every size in the windows, in one place to tweak                          |
| `src/state.rs`      | Follower thread, config and status (ported from the Tauri app)            |
| `src/observer.rs`   | Upload counts (copied unchanged from the Tauri app)                       |

Dependencies: `gpui-kit` 0.9.0, which pins its GPUI snapshot (`gpui-pre` 0.3.11).

## Not done yet

- Bundling: no `.app`, signing, notarization or universal build. The release workflow still ships
  the Tauri app.
- UI tests (GPUI has a headless harness).
- Selecting and copying log text.
