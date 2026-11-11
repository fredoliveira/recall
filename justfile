# recall task runner (`brew install just`). `just` lists recipes.
# Plain builds are just cargo: `cargo build`, `cargo test`, `cargo clean`, …

# Show available recipes.
default:
    @just --list

# --- Run ---------------------------------------------------------------------

# Extra args pass through: `just run path/to/Player.log` parses that file once.
# Build and run the CLI, following the auto-detected Player.log.
run *ARGS:
    cargo run -p recall -- {{ARGS}}

# Replay a log through the client offline (prints payloads, uploads nothing).
replay LOG:
    cargo run -p recall-core --example replay -- {{LOG}}

# Check parser output against golden snapshots of the real local logs (refactor net;
# baselines are written on first run and live under gitignored local/golden/).
golden:
    for f in local/logs/*.log; do \
        cargo run -q -p recall-core --example golden -- "$f" "local/golden/$(basename "$f" .log).jsonl" || exit 1; \
    done

# --- Desktop app -------------------------------------------------------------
# Its own workspace with a pinned toolchain, named explicitly because a global RUSTUP_TOOLCHAIN
# (mise sets one) overrides rust-toolchain.toml.

desktop_toolchain := `sed -n 's/^channel = "\(.*\)"$/\1/p' crates/desktop/rust-toolchain.toml`

# Start the mock first so it never hits the live API:
# `python3 tools/mock_server.py 8732 /tmp/desktop-out.jsonl`.
# Run the desktop app against the local mock on :8732.
desktop-dev:
    cd crates/desktop && RECALL_HOST=http://127.0.0.1:8732 cargo +{{desktop_toolchain}} run

# Uploads to the live host, like the installed app (quit that one first).
# Build and run the desktop app (release).
desktop-run:
    cd crates/desktop && cargo +{{desktop_toolchain}} run --release

# Build dist/Recall.app and the .dmg, signed (see crates/desktop/scripts/bundle.sh).
desktop-build:
    crates/desktop/scripts/bundle.sh --universal --dmg

# --- Quality -----------------------------------------------------------------

# Run the full test suite.
test:
    cargo test

# Reflow all Markdown to 100 columns (Prettier via npx; config in .prettierrc).
fmt-md:
    npx --yes prettier@3 --write "**/*.md"

# Pre-commit gate, mirroring CI: format check + clippy (all crates) + tests.
lint:
    cargo fmt --check
    npx --yes prettier@3 --check "**/*.md"
    cargo clippy --all-targets -- -D warnings
    cd crates/desktop && cargo +{{desktop_toolchain}} fmt --check
    cd crates/desktop && cargo +{{desktop_toolchain}} clippy --locked --all-targets -- -D warnings
    cargo test
