#!/bin/sh
# Builds dist/Recall.app from this crate, signed with your Developer ID (hardened runtime and
# a secure timestamp, as notarization requires).
#
#   scripts/bundle.sh                    this machine's architecture
#   scripts/bundle.sh --universal        Apple Silicon + Intel in one binary (what ships)
#   scripts/bundle.sh --universal --dmg  also dist/Recall-universal.dmg
#   scripts/bundle.sh --dmg-only         wrap the dist/Recall.app already there, untouched
#
# --dmg-only is for the release workflow, which notarizes and staples the app between the two
# steps so the app inside the disk image carries its own ticket.
#
# RECALL_SIGN_IDENTITY picks the certificate; without it the first "Developer ID Application"
# in the keychain is used, and with none at all the app is signed ad hoc (runs here, cannot
# be notarized). Notarizing is the release workflow's job, not this script's.
set -eu

universal=false
dmg=false
build=true
for arg in "$@"; do
  case "$arg" in
    --universal) universal=true ;;
    --dmg) dmg=true ;;
    --dmg-only) dmg=true build=false ;;
    *) echo "usage: bundle.sh [--universal] [--dmg | --dmg-only]" >&2; exit 2 ;;
  esac
done

crate=$(cd "$(dirname "$0")/.." && pwd)
dist="$crate/dist"
app="$dist/Recall.app"
bundle_id="com.helloform.recall"
bin="recall-gpui"
toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$crate/rust-toolchain.toml")
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -n1)

# LSUIElement: a menu-bar app, with no Dock icon from the first moment of launch.
write_info_plist() {
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Recall</string>
  <key>CFBundleDisplayName</key><string>Recall</string>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleExecutable</key><string>Recall</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
}

cd "$crate"
identity=${RECALL_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -n1)}

if $build; then
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

  if $universal; then
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
      echo "Building for ${target}…"
      cargo "+$toolchain" build --release --locked --target "$target"
    done
    lipo -create -output "$app/Contents/MacOS/Recall" \
      "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
  else
    echo "Building…"
    cargo "+$toolchain" build --release --locked
    cp "target/release/$bin" "$app/Contents/MacOS/Recall"
  fi

  cp "$crate/assets/icon.icns" "$app/Contents/Resources/icon.icns"

  write_info_plist

  if [ -n "$identity" ]; then
    echo "Signing as ${identity}…"
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
  else
    echo "No Developer ID certificate found: signing ad hoc (cannot be notarized)." >&2
    codesign --force --sign - "$app"
  fi
  codesign --verify --strict --verbose=2 "$app"
  echo "Done: $app ($(du -sh "$app" | cut -f1))"
else
  test -d "$app" || { echo "no $app to wrap: build it first" >&2; exit 1; }
fi

if $dmg; then
  out="$dist/Recall-universal.dmg"
  staging=$(mktemp -d)
  trap 'rm -rf "$staging"' EXIT
  cp -R "$app" "$staging/Recall.app"
  ln -s /Applications "$staging/Applications"
  rm -f "$out"
  hdiutil create -volname Recall -srcfolder "$staging" -fs HFS+ -format UDZO -quiet "$out"
  if [ -n "$identity" ]; then
    codesign --force --timestamp --sign "$identity" "$out"
  fi
  echo "Done: $out ($(du -sh "$out" | cut -f1))"
fi
