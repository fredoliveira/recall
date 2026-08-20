# tests/fixtures

Sanitized MTGA `Player.log` snippets + expected payloads, consumed by `tests/parity.rs` (synthetic
gap branches) and `tests/parity_game.rs` (real-game `add_game`). Together they pin all 15 endpoints
byte-for-byte - this is the wire contract and the refactor net.

| Fixture                            | Test             | Covers                                                                                                                                                                                                                 |
| ---------------------------------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `gaps.log`                         | `parity.rs`      | 14 endpoints via synthetic entries: account, bot draft pack/pick, combined human draft pack/pick, event ended, event course, inventory, collection, event join, deck submission, ongoing events, rank, player progress |
| `game.log` + `expected/game.jsonl` | `parity_game.rs` | `add_game` (full game-state machine: 34 top-level keys, 198 history events) + `add_mtga_account`; sanitized excerpt of a real `Traditional_Ladder` match                                                               |

Expected payloads were derived from the upstream Python source and/or captured from the
oracle-validated implementation. `expected/game.jsonl` is a snapshot of that implementation's output
(exact Python `json.dumps` bytes, compared raw so the formatter is under test); it is canonical
under `TZ=UTC` (the envelope `utc_time` is local-timezone-dependent and asserted by shape, not
bytes).

## Second net: golden snapshots over real logs (local-only)

`just golden` replays the raw logs under `local/logs/` (gitignored - they contain account id /
screen names / token, **never commit them or anything derived from them without sanitizing**) and
byte-compares every submission against baselines in `local/golden/`. First run writes baselines; use
before/after any refactor for full real-data coverage (~250 submissions, all endpoints present in
the real logs).

## Adding fixtures

Synthetic entries: mimic the `gaps.log` line format, derive expectations from the upstream handler
source (exact field order, null-vs-absent, coercions). Real excerpts: sanitize first (display name,
account id, token, all screen-name occurrences, and every UUID - match, session, transaction and
request ids - mapped consistently to synthetic values; grep to verify zero hits), then generate
expectations by replaying the sanitized excerpt.
