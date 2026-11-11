# CLAUDE.md

## What this is

**Recall** (after Ancestral Recall) is an MTG Arena log client: one parser that tails `Player.log`
and turns it into a neutral event stream, and pluggable **services** that translate those events
into uploads. 17Lands is the default service - on the wire Recall is a drop-in for the official
Python client (`seventeenlands` 0.1.44): same payloads, same endpoints, byte-for-byte.

Upstream: https://github.com/rconroy293/mtga-log-client. We send its `CLIENT_VERSION = "0.1.44.p"`
verbatim so the server treats us as the trusted Python client.

## Prime directive: honor the wire contract

The contract is **the bytes the 17Lands adapter emits**, pinned by fixtures - not the Python
source's structure. Refactor freely; never change emitted bytes except to track an upstream change.
Value-affecting oddities inherited from upstream are marked `WIRE:` in the code
(`grep -rn "WIRE:" crates/core/src`) - each is load-bearing, however strange:

- Dispatch is first-match-wins; the chain's **order** is semantics (`follower/dispatch.rs`).
- `serde_json` has `preserve_order`; payloads are emitted with a Python-`json.dumps` formatter
  (`services::seventeenlands::to_python_json_vec`), so key order + spacing match.
- Null vs absent: payloads are `Map`/`Value` and **emit `null`** (never `skip_serializing_if`, never
  serde structs for payloads).
- Time formatting mimics Python `datetime.isoformat()`; `.NET` ticks reproduce Python's **lossy
  f64** division (`time_parse::from_dotnet_ticks`).
- `Map::shift_remove` mirrors `dict.pop` (order-preserving); `remove` is `swap_remove`.
- The game-history `_timestamp` comes from `EventTime` (usually absent → `null`) - an upstream
  variable-reuse quirk. `defaultdict` semantics (e.g. `mulligan_count` can be -1), `int()`
  coercions, Python truthiness and `str()` rendering are reproduced where marked.

## Architecture

Three crates under `crates/`: a Cargo workspace of core + cli, and the desktop app as its own
workspace:

- **`core`** (`recall-core`)
    - `follower.rs` + `follower/` - the parser (port of upstream's `Follower` class). `follower.rs`:
      state + reset lifecycle (`UserInfo` / `MatchState` / `GameState`, dropped wholesale to reset);
      `tail.rs` follow loop; `dispatch.rs` blob parse + dispatch table; `account.rs` / `draft.rs` /
      `event.rs` / `game.rs` handlers. Emits `GameEvent`s; knows no service.
    - `events.rs` - `GameEvent` (typed, credential-free, defined by what the log contains),
      `Context` (who/when), the `Service` trait (`handle(&GameEvent)`).
    - `services/seventeenlands.rs` + `seventeenlands/` - **the parity surface**. The adapter owns
      the token, `CLIENT_VERSION`, `DEFAULT_HOST`, endpoint/gzip routing and _all_ 17Lands payload
      shaping; `seventeenlands/transport.rs` is the wire layer (`Submitter` trait, HTTP `ApiClient`,
      python-JSON, gzip), re-exported at the service level.
    - `services/archive.rs` - local JSONL record; the minimal secondary service.
      `services/fanout.rs` - N services in order. `services.rs` - `from_config`, the one place both
      binaries build their service set.
    - `config.rs` - typed `Config` (`[services.<name>]` sections); `load()` migrates the flat
      top-level token. `resolve_token` (CLI): config only, else print where the token goes and
      exit - no flag, no prompt, no legacy-ini import.
    - `checkpoint.rs` - how far into each log the services have been fed
      (`<data_dir>/recall/state.json`); see "Resuming" below.
    - `paths.rs` log discovery · `retry.rs` · `time_parse.rs`.
    - `tests/` parity fixtures (see `tests/fixtures/README.md`), `examples/` `replay`, `golden`,
      `replay_archive`.
- **`cli`** (`recall`) - `main.rs`: config + token resolution, logging, processing loop.
- **`desktop`** (`recall-desktop`) - GPUI (gpui-kit) macOS menu-bar app on the same core; observes
  uploads via an `ObservingSubmitter` decorator under the 17Lands adapter. Every size lives in
  `src/style.rs`. See its README.

The desktop crate is excluded from the workspace (own lockfile, pinned toolchain in its
`rust-toolchain.toml`), so bare `cargo` commands skip it; build it from `crates/desktop` with
`cargo +<that toolchain>` or the `just desktop-*` recipes. CI builds and clippies it separately.

### The event model

`GameEvent` variants are occurrences the log defines (a pack was seen, a game completed), never
things a service's API wants. Rules when adding or changing one:

- Type what the log guarantees: integers the log always carries are `i64`; ids and names it carries
  loosely pass through as `Value` (strings in practice). Attach the raw MTGA blob (`raw`) where one
  exists, so no adapter is ever starved.
- Every variant carries a `Context`; `GameEvent::name()` is its neutral snake_case id and
  `to_json()` its generic view (context fields, then data) - that view is what the archive writes,
  so an incompatible change bumps `archive::FORMAT_VERSION`.
- A new variant touches: `events.rs` (variant, `name`, `context`, `to_json`), the follower handler
  that emits it, the 17Lands adapter (`route` + `body` arm mirroring the upstream handler), and a
  fixture pinning the bytes.

### Resuming

A restart must not re-upload the log, but the follower cannot seek: its state is built from
everything before the offset. So resuming is a **muted replay** (`tail.rs`, `parse_log_resumable`):
read from byte 0, run every handler, drop the events (`Follower::emit` is the single emit point)
until the checkpoint, then go live. Rules:

- Handlers must stay deterministic functions of the log - no wall clock, no I/O - or the replayed
  state diverges from the first run's. `tests/resume.rs` pins "interrupted + resumed == one full
  run", byte for byte.
- The checkpoint only moves past events the primary service accepted (`Service::handle` returns
  whether the event was delivered), so delivery is at-least-once.
- A checkpoint matches a file by the bytes just before its offset, not by name (a rotated
  `Player-prev.log` resumes), and is scoped to host + token.
- `parse_log` / `recall <LOG>` / the examples never read or write checkpoints.

### Adding a service

Implement `events::Service` and add it in `services::from_config` behind a `[services.<name>]`
config section (`config.rs`). Rules: a secondary returns `true` from `handle` (only the primary may
hold the checkpoint back); a service only sees `&GameEvent` - never another service's credentials;
only the 17Lands leg gets blocking retries; anything secondary must never stall or fail the loop (no
retries; `services::archive::Archive` is the template: append, log once on error, keep going).

## Verifying

Two nets; both must stay **byte-identical** through any change that isn't an upstream port:

1. `cargo test` - `tests/parity.rs` (`fixtures/gaps.log`, 14 endpoints, synthetic) and
   `tests/parity_game.rs` (`fixtures/game.log`, a sanitized real match → `add_game`).
2. `just golden` - replays the real logs in gitignored `local/logs/` against baselines in
   `local/golden/` (~250 submissions). Baselines are written on first run; regenerate only after an
   intentional wire change.

`just lint` (fmt check, `clippy --all-targets -D warnings`, tests) mirrors CI and must be clean
before a commit; it covers the desktop crate too.

## Tracking upstream

The local reference copy is gone; diff against GitHub directly. When upstream releases:

1. Diff `src/python/seventeenlands/{mtga_follower,api_client}.py` between the ported ref and the new
   one (`https://raw.githubusercontent.com/rconroy293/mtga-log-client/<ref>/…`).
2. Port _behavior_: parsing/state changes go in `follower/`, payload-shape changes in
   `services/seventeenlands/`; bump `seventeenlands::CLIENT_VERSION` if theirs changed.
3. Derive new/changed fixture expectations from the Python semantics (order, null-vs-absent,
   coercions), run both nets, regenerate golden baselines, record the new ported ref here.

## Conventions

- **Never POST to the live `api.17lands.com`** during development without explicit approval; use the
  local mock (`tools/mock_server.py`).
- Docs are crisp and minimal; the justfile holds only non-obvious commands. Markdown is
  Prettier-formatted at 100 columns (`just fmt-md`; `just lint` and CI check it).
- 2018-style module files (`foo.rs` + `foo/`), never `mod.rs`. Follower handlers are `pub(super)`
  methods spread over `follower/*.rs`. Unit tests needing a `Context` use `Context::test_fixture()`.
- Work lands on feature branches in small, individually green commits; mechanical multi-file steps
  are delegated to fresh-context subagents with both nets as hard gates, then verified independently
  before being relayed.
- Deliberate deviations from upstream: per-service config at `<config_dir>/recall/config.toml`
  (`services.seventeenlands.token`; a flat `token` auto-migrates), no token prompt of any kind, no
  startup version check, no server-side error reporting, stdout/stderr logging only.
- Rate limiting (429, or a 403 from the `awselb` firewall) is retried like a 5xx; upstream drops
  those events. 4xx rejections log at WARN.
- Naming: the product is Recall; nothing that reaches the 17Lands server is renamed (payloads,
  `CLIENT_VERSION`, endpoints), and "17Lands" stays wherever it means the service.
- Non-wire seams for the desktop app: `config::Config::{load, save}` and `services::from_config` are
  shared with the CLI; `Follower::parse_log_cancellable` adds cancellation and `parse_log_resumable`
  checkpoints (`parse_log` delegates to them, byte-identical).
