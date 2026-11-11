# Homebrew tap

The tap ([fredoliveira/homebrew-tap](https://github.com/fredoliveira/homebrew-tap)) ships two
things, both built and attached to each GitHub release:

- **`Formula/recall.rb`** - a binary "pour" **formula** for the `recall` CLI. Covers macOS (arm64 +
  x86_64) and Linux (x86_64); Windows isn't a Homebrew target.
- **`Casks/recall-desktop.rb`** - a **cask** for the menu-bar desktop app (a universal `.dmg`,
  Intel + Apple Silicon). macOS only.

Both tap files are **generated** by [`generate.sh`](generate.sh) here - edit the generator, never
the tap files directly.

> Signing: the desktop app is signed with a Developer ID certificate and notarized by Apple in the
> Release Desktop workflow (the app and the `.dmg` are both stapled), so the cask installs and
> launches without a Gatekeeper prompt. A tagged release fails rather than ship unsigned.
>
> The CLI binary is only ad-hoc signed. That is fine through Homebrew, which downloads via `curl`
> and so never applies the `com.apple.quarantine` xattr; a tarball downloaded in a browser is
> quarantined and blocked until the attribute is cleared.

### Signing secrets

The Release Desktop workflow needs these repo secrets:

| Secret                       | Value                                                             |
| ---------------------------- | ----------------------------------------------------------------- |
| `APPLE_CERTIFICATE`          | the Developer ID Application certificate + key (`.p12`), base64   |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password                                               |
| `APPLE_SIGNING_IDENTITY`     | the identity name, `Developer ID Application: <name> (<team id>)` |
| `APPLE_API_ISSUER`           | App Store Connect API issuer ID                                   |
| `APPLE_API_KEY`              | App Store Connect API key ID                                      |
| `APPLE_API_KEY_P8`           | contents of the API key's `.p8` file                              |

The certificate expires after five years and the workflow starts failing when it does; re-export and
replace the first two. To test signing without publishing, run the workflow manually
(`gh workflow run release-desktop.yml --ref <branch>`): it builds and notarizes the `.dmg` as an
artifact only.

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
