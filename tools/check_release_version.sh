#!/usr/bin/env sh
# Release gate: the tag being released must match the workspace version in Cargo.toml and the
# desktop crate's own version in crates/desktop/Cargo.toml (it is outside the workspace, and its
# version is stamped into the .app).
#
#   check_release_version.sh v0.3.1
set -eu

tag="${1:?usage: check_release_version.sh <tag>}"
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
desktop_version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/desktop/Cargo.toml | head -n1)"

fail=0
[ -n "$cargo_version" ] || { echo "could not read the version from Cargo.toml" >&2; fail=1; }
[ "v${cargo_version}" = "$tag" ] || { echo "tag ${tag} != Cargo.toml version ${cargo_version}" >&2; fail=1; }
[ "$desktop_version" = "$cargo_version" ] || {
  echo "crates/desktop version ${desktop_version} != Cargo.toml version ${cargo_version}" >&2
  fail=1
}
[ "$fail" -eq 0 ] || exit 1
echo "release version ${cargo_version} is consistent"
