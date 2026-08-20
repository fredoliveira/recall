# Homebrew tap

The tap ([fredoliveira/homebrew-tap](https://github.com/fredoliveira/homebrew-tap)) ships two
things, both built and attached to each GitHub release:

- **`Formula/recall.rb`** - a binary "pour" **formula** for the `recall` CLI. Covers macOS (arm64 +
  x86_64) and Linux (x86_64); Windows isn't a Homebrew target.
- **`Casks/recall-desktop.rb`** - a **cask** for the menu-bar desktop app (a universal `.dmg`,
  Intel + Apple Silicon). macOS only.

Both tap files are **generated** by [`generate.sh`](generate.sh) here - edit the generator, never
the tap files directly.

> Note on notarization (CLI): Homebrew downloads via `curl`, which does **not** apply the
> `com.apple.quarantine` xattr, so Gatekeeper never blocks the brew-installed CLI even though the
> binary is only ad-hoc signed (not Apple-notarized).
>
> The **cask is different**: `brew install --cask` quarantines the `.app`, so the unsigned desktop
> app is blocked by Gatekeeper on first launch. Until the app is Apple-notarized, users must open it
> once via right-click → Open (or run `xattr -dr com.apple.quarantine "/Applications/Recall.app"`).
> The cask file documents this. Notarization needs an Apple Developer ID signing cert +
> `xcrun notarytool` in the Release Desktop workflow, after which the cask "just works".

Users install:

```sh
brew install fredoliveira/tap/recall            # the CLI
brew install --cask fredoliveira/tap/recall-desktop  # the menu-bar app
```

## On every release: automated

Each release workflow ends with a `tap` job that regenerates its tap file from the just-built assets
and pushes to the tap repo:

- `release.yml` → `Formula/recall.rb` (from the three unix tarballs)
- `release-desktop.yml` → `Casks/recall-desktop.rb` (from the `.dmg`)

The jobs authenticate with the **`TAP_PUSH_TOKEN`** repo secret - a fine-grained PAT with
`contents: write` on `fredoliveira/homebrew-tap`. If the secret is missing the jobs skip quietly and
the bump must be done by hand (below).

## Manual bump (fallback)

Download the assets from the release, checksum them, and regenerate:

```sh
gh release download vX.Y.Z -R fredoliveira/recall -D /tmp/rel
shasum -a 256 /tmp/rel/*

packaging/homebrew/generate.sh formula X.Y.Z <sha-arm64-mac> <sha-x86_64-mac> <sha-x86_64-linux> \
  > ../homebrew-tap/Formula/recall.rb
packaging/homebrew/generate.sh cask X.Y.Z <sha-dmg> \
  > ../homebrew-tap/Casks/recall-desktop.rb
```

Commit and push the tap repo.

## Verify before publishing

From within the tap repo:

```sh
brew audit --strict --online Formula/recall.rb
brew install Formula/recall.rb                  # binary pour
brew test recall

brew audit --strict --online --cask Casks/recall-desktop.rb
brew install --cask Casks/recall-desktop.rb
```
