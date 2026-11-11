# Recall desktop

A Tauri v2 menu-bar / notification-area app that wraps the Rust log follower core. The follower runs
on a background thread, shows a live log feed in an on-demand window, and captures the 17Lands token
through a GUI; it reads and writes the same `config.toml` as the CLI.

## Install

```sh
brew install --cask fredoliveira/tap/recall-desktop
```

The release workflow attaches a universal `.dmg` (Intel + Apple Silicon), signed with a Developer ID
and notarized by Apple. See [`packaging/homebrew/README.md`](../../packaging/homebrew/README.md).

## Architecture

| File               | Role                                                                                |
| ------------------ | ----------------------------------------------------------------------------------- |
| `src/main.rs`      | App bootstrap, tray (menu-bar, accessory mode), window toggle, command registration |
| `src/state.rs`     | `AppState`: follower thread + cancellation, resolved log path, shared upload status |
| `src/observer.rs`  | `ObservingSubmitter` - records endpoint/time/count without altering POSTs           |
| `src/logbridge.rs` | Global `log::Log` sink → ring buffer + `log-line` webview events                    |
| `src/commands.rs`  | `#[tauri::command]` bridge (token, status, log path)                                |
| `ui/`              | Vanilla HTML/JS/CSS frontend (no bundler; uses `withGlobalTauri`)                   |

## Develop

```sh
# Never POST to the live API during dev - point at the local mock server.
python3 ../../tools/mock_server.py 8732 /tmp/desktop-out.jsonl &
RECALL_HOST=http://127.0.0.1:8732 cargo tauri dev
```

Dev/test env overrides (parallel to each other):

- `RECALL_HOST` - upload host override (default: `services.seventeenlands.host` from the config,
  else the live API). **Always set this to the mock in dev.**
- `RECALL_LOG` - pin the followed log file (e.g. `../core/tests/fixtures/gaps.log`) so the app can
  run headlessly without a real `Player.log`.

## Build from source

Prerequisites:

- **Rust** (stable) - <https://rustup.rs>
- **Linux only**: a system webview toolkit. On Debian/Ubuntu:
    ```sh
    sudo apt install libwebkit2gtk-4.1-dev build-essential libxdo-dev \
      libssl-dev libayatana-appindicator3-dev librsvg2-dev
    ```
    (see <https://tauri.app/start/prerequisites/> for other distros). macOS/Windows need nothing
    extra.

**Easiest - build and run** (no extra tooling; the UI is embedded, so the binary is self-contained).
From the repo root:

```sh
cargo build -p recall-desktop --release
./target/release/recall-desktop        # launches the menu-bar app
```

Or via the task runner: `just desktop-run`.

**Build a distributable bundle** (`.app`/`.dmg`/`.msi`) - this one needs the Tauri CLI:

```sh
cargo install tauri-cli --locked
cd crates/desktop && cargo tauri build         # bundles under target/release/bundle/
```

## TODO

- Windows (`.msi`/NSIS) targets can be added to `tauri.conf.json` later.
