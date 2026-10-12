#!/usr/bin/env bash
# Build Navi.app for macOS, sign it with Developer ID, notarize + staple it,
# and package the stapled app as Navi-<version>-<arch>.zip.
#
# Usage:
#   scripts/build-macos.sh                # release build, host arch (named by it: arm64)
#   ARCH=universal scripts/build-macos.sh # arm64 + x86_64 universal binary
#
# Output: dist/Navi.app  and  dist/Navi-<version>-<arch>.zip (an ad-hoc build
# is packaged as Navi-<version>-<arch>-adhoc.zip, never under the release name)
#
# Environment:
#   NAVI_SIGN_ID              codesign identity (unset: the first installed
#                             "Developer ID Application"; none found, or set
#                             but empty (NAVI_SIGN_ID=): ad-hoc, no
#                             notarization, NOT release-grade)
#   NAVI_NOTARY_PROFILE       notarytool keychain profile (default: tangent-notary)
#   NAVI_ALLOW_UNNOTARIZED=1  package a signed app even when notarization failed
#   CARGO_TARGET_DIR          respected; defaults to ~/.cache/navi-target when
#                             the repo lives in Dropbox (it breaks cargo linking)
#   MACOSX_DEPLOYMENT_TARGET  default 14.0; also written as LSMinimumSystemVersion
#
# Requirements: Rust toolchain (rustup), Xcode CLT for codesign / ditto /
# notarytool / stapler. For ARCH=universal:
#   rustup target add x86_64-apple-darwin aarch64-apple-darwin
# The notary profile is created once with:
#   xcrun notarytool store-credentials tangent-notary \
#     --apple-id <id> --team-id 6GFLLR9599 --password <app-specific-password>
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCH="${ARCH:-host}"
VERSION="$(grep '^version' navi/Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
# Info.plist version keys take at most three period-separated integers; a
# semver suffix ("1.1.0-beta.1", "1.1.0+ci7") is invalid there and notarization
# rejects it. File names keep the full string, the bundle gets the numeric core.
VERSION_NUM="${VERSION%%-*}"; VERSION_NUM="${VERSION_NUM%%+*}"
APP="dist/Navi.app"
# "host" names the zip after the machine's real arch, so nobody downloads an
# arm64-only "host" build for an Intel Mac.
ZIP_ARCH="$ARCH"; [ "$ARCH" != host ] || ZIP_ARCH="$(uname -m)"
ZIP="dist/Navi-${VERSION}-${ZIP_ARCH}.zip"
BUNDLE_ID="com.navi.graph"   # unchanged since 1.0: renaming it orphans prefs and TCC grants

# One deployment target for both slices and the plist. rustc's defaults differ
# per arch (aarch64 11.0, x86_64 10.12), which would leave a universal binary
# disagreeing with its own LSMinimumSystemVersion. 14.0 because the frame
# pacer (macos_display.rs) calls NSScreen displayLinkWithTarget:selector: and
# CADisplayLink, both macOS 14: on 11-13 they'd abort at launch, so the
# plist must keep those systems from opening it at all.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

# Cargo cannot link build scripts inside Dropbox ("Operation not permitted").
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  case "$ROOT" in
    *Dropbox*) export CARGO_TARGET_DIR="$HOME/.cache/navi-target" ;;
    *)         export CARGO_TARGET_DIR="$ROOT/target" ;;
  esac
fi
TARGET_DIR="$CARGO_TARGET_DIR"

echo "==> Navi $VERSION (bundle $VERSION_NUM)  arch=$ARCH  target-dir=$TARGET_DIR"

# ── Build ────────────────────────────────────────────────────────────────────
case "$ARCH" in
  host)
    cargo build --release -p navi
    BIN="$TARGET_DIR/release/navi"
    ;;
  universal)
    # Homebrew rust shadows rustup's cargo on PATH and has no x86_64 std, so
    # the cross target would not resolve; prefer the rustup toolchain here.
    TOOLCHAIN_BIN="$(rustup which cargo 2>/dev/null | xargs dirname || true)"
    [ -n "$TOOLCHAIN_BIN" ] || TOOLCHAIN_BIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
    [ ! -d "$TOOLCHAIN_BIN" ] || export PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"
    rustup target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null 2>&1 || true
    cargo build --release -p navi --target x86_64-apple-darwin
    cargo build --release -p navi --target aarch64-apple-darwin
    mkdir -p "$TARGET_DIR/universal/release"
    BIN="$TARGET_DIR/universal/release/navi"
    lipo -create -output "$BIN" \
      "$TARGET_DIR/x86_64-apple-darwin/release/navi" \
      "$TARGET_DIR/aarch64-apple-darwin/release/navi"
    ;;
  *)
    echo "unknown ARCH=$ARCH (expected host|universal)" >&2
    exit 2
    ;;
esac

# ── Bundle ───────────────────────────────────────────────────────────────────
echo "==> Assembling $APP"
# The bundle only: the old zip stays until a new one is ready to replace it,
# so a failed sign or notarization does not destroy the last good artifact
# (an ad-hoc build never replaces it either: it is packaged under -adhoc).
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/navi"
chmod +x "$APP/Contents/MacOS/navi"
strip -x "$APP/Contents/MacOS/navi" 2>/dev/null || true

# Icon: build a fully canonical .icns from the highest-resolution variant
# we can find, scaling with `sips` to every size Finder/Dock expects. We
# write the icns binary ourselves rather than going through iconutil because
# `iconutil` on recent macOS has been reported to reject perfectly valid
# iconsets with the "Invalid Iconset" error when files have any extended
# attributes the tool didn't write itself.
if [ -f assets/icon.icns ] || [ -f assets/icon.png ]; then
  TMP_DIR="$(mktemp -d)"
  PNG_DIR="$TMP_DIR/png"
  mkdir -p "$PNG_DIR"

  if [ -f assets/icon.png ]; then
    SOURCE="assets/icon.png"
  else
    iconutil -c iconset -o "$TMP_DIR/extracted.iconset" assets/icon.icns >/dev/null 2>&1
    if [ -f "$TMP_DIR/extracted.iconset/icon_512x512@2x.png" ]; then
      SOURCE="$TMP_DIR/extracted.iconset/icon_512x512@2x.png"
    elif [ -f "$TMP_DIR/extracted.iconset/icon_512x512.png" ]; then
      SOURCE="$TMP_DIR/extracted.iconset/icon_512x512.png"
    else
      SOURCE=""
    fi
  fi

  if [ -n "$SOURCE" ]; then
    # Render every variant size as a clean PNG via sips.
    for spec in 16:16.png 32:32.png 64:64.png 128:128.png 256:256.png \
                512:512.png 1024:1024.png; do
      px="${spec%%:*}"; name="${spec##*:}"
      sips -z "$px" "$px" -s format png --out "$PNG_DIR/$name" "$SOURCE" >/dev/null
    done

    # Pack into a canonical icns. The chunk-type → pixel-size mapping is
    # the standard one Apple's docs document (icp4 = 16, icp5 = 32, etc.).
    /usr/bin/python3 - "$PNG_DIR" "$APP/Contents/Resources/AppIcon.icns" <<'PY'
import os, struct, sys
src, out = sys.argv[1], sys.argv[2]
chunks = [
    ('icp4', '16.png'),    # 16x16
    ('icp5', '32.png'),    # 32x32
    ('icp6', '64.png'),    # 64x64
    ('ic07', '128.png'),   # 128x128
    ('ic08', '256.png'),   # 256x256
    ('ic09', '512.png'),   # 512x512
    ('ic10', '1024.png'),  # 512x512@2x
    ('ic11', '32.png'),    # 16x16@2x
    ('ic12', '64.png'),    # 32x32@2x
    ('ic13', '256.png'),   # 128x128@2x
    ('ic14', '512.png'),   # 256x256@2x
]
body = b''
for code, fname in chunks:
    data = open(os.path.join(src, fname), 'rb').read()
    body += code.encode('ascii') + struct.pack('>I', len(data) + 8) + data
total = len(body) + 8
header = b'icns' + struct.pack('>I', total)
open(out, 'wb').write(header + body)
print(f"  icns: {len(chunks)} chunks, {total} bytes -> {out}")
PY
  else
    cp assets/icon.icns "$APP/Contents/Resources/AppIcon.icns"
  fi
  rm -rf "$TMP_DIR"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>             <string>Navi</string>
    <key>CFBundleDisplayName</key>      <string>Navi</string>
    <key>CFBundleExecutable</key>       <string>navi</string>
    <key>CFBundleIconFile</key>         <string>AppIcon</string>
    <key>CFBundleIconName</key>         <string>AppIcon</string>
    <key>CFBundleIdentifier</key>       <string>${BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key> <string>6.0</string>
    <key>CFBundlePackageType</key>      <string>APPL</string>
    <key>CFBundleShortVersionString</key> <string>${VERSION_NUM}</string>
    <key>CFBundleVersion</key>          <string>${VERSION_NUM}</string>
    <key>LSMinimumSystemVersion</key>   <string>${MACOSX_DEPLOYMENT_TARGET}</string>
    <key>NSHighResolutionCapable</key>  <true/>
    <key>LSApplicationCategoryType</key> <string>public.app-category.utilities</string>
    <key>NSPrincipalClass</key>         <string>NSApplication</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# ── Sign ─────────────────────────────────────────────────────────────────────
# Developer ID + hardened runtime + notarization is what lets a downloaded app
# open without the "Open Anyway" trip through System Settings (the right-click
# Open bypass is gone as of macOS 15).
#
# No entitlements file, on purpose. The hardened runtime would need one for
# JIT / unsigned executable memory, DYLD env vars, third-party dylibs, or
# camera / mic / Apple Events; Navi uses none of them. It links only system
# frameworks (OpenGL through glutin's CGL path is Apple-signed, so library
# validation passes), and spawning emacsclient is plain fork/exec of another
# signed-or-not binary, which the hardened runtime does not restrict: the child
# runs under its own signature, and the server socket it talks to is ordinary
# file-system IPC (no sandbox here).
# `-` not `:-`: NAVI_SIGN_ID= (set, empty) asks for an ad-hoc build on purpose.
SIGN_ID="${NAVI_SIGN_ID-$(security find-identity -v -p codesigning 2>/dev/null \
  | grep "Developer ID Application" | head -1 \
  | sed -E 's/.*\) ([0-9A-F]{40}) ".*/\1/' || true)}"

# Dropbox and Finder stamp xattrs that codesign rejects as "detritus".
xattr -cr "$APP" 2>/dev/null || true

# Inside-out rather than --deep (deprecated: it applies the outer bundle's
# options to nested code). There is one Mach-O today; sign it, then the bundle.
if [ -n "$SIGN_ID" ]; then
  echo "==> Signing with Developer ID ($SIGN_ID)"
  codesign --force --options runtime --timestamp -s "$SIGN_ID" "$APP/Contents/MacOS/navi"
  codesign --force --options runtime --timestamp -s "$SIGN_ID" "$APP"
  SIGNED_RELEASE=1
else
  echo ""
  echo "!! WARNING: no Developer ID Application certificate found; signing AD-HOC."
  echo "!! This build is NOT notarized and WILL be refused by Gatekeeper on any"
  echo "!! other Mac. Do not publish it."
  echo ""
  # Hardened runtime here too, so a local build behaves like the release one.
  codesign --force --options runtime -s - "$APP/Contents/MacOS/navi"
  codesign --force --options runtime -s - "$APP"
  SIGNED_RELEASE=0
  # never under the release name: the last notarized zip stays where it is
  ZIP="dist/Navi-${VERSION}-${ZIP_ARCH}-adhoc.zip"
fi
codesign --verify --strict --deep --verbose=2 "$APP"
# The hardened runtime is a notarization prerequisite; read it back out of the
# signature rather than trusting the flag. (No `grep -q`: it exits at the first
# match and SIGPIPEs codesign under pipefail.)
case "$(codesign -d -v "$APP" 2>&1 || true)" in
  *"flags="*"runtime)"*) ;;   # "(runtime)" or "(adhoc,runtime)"
  *) echo "error: signed bundle does not have the hardened runtime" >&2; exit 1 ;;
esac

# ── Notarize ─────────────────────────────────────────────────────────────────
# Notarize and staple BEFORE packaging, so the zip carries the stapled app and
# a first launch works offline. Stapling the zip itself does nothing.
NOTARY_PROFILE="${NAVI_NOTARY_PROFILE:-tangent-notary}"
NOTARIZED=0
if [ "$SIGNED_RELEASE" = 1 ]; then
  NOTARY_DIR="$(mktemp -d)"
  NOTARY_LOG="$NOTARY_DIR/submit.log"
  SUBMISSION_ID=""
  echo "==> Notarizing with profile '$NOTARY_PROFILE' (waits on Apple; typically 1-5 minutes)"
  # No xattrs or resource forks in either zip: dist/ is in Dropbox, which
  # re-stamps its own (com.dropbox.*) on every file seconds after `xattr -cr`.
  # Nothing they hold is sealed; the staple ticket is a file in the bundle.
  ditto -c -k --norsrc --noextattr --keepParent "$APP" "$NOTARY_DIR/Navi.zip"
  # `submit --wait` can exit 0 on an Invalid verdict, so the exit status is not
  # the answer: read the id and status out of the log, then let the stapler and
  # spctl below have the final word.
  xcrun notarytool submit "$NOTARY_DIR/Navi.zip" \
    --keychain-profile "$NOTARY_PROFILE" --wait 2>&1 | tee "$NOTARY_LOG" || true
  SUBMISSION_ID="$(awk '$1 == "id:" { print $2; exit }' "$NOTARY_LOG")"
  STATUS="$(awk '$1 == "status:" { s = $2 } END { print s }' "$NOTARY_LOG")"
  if [ "$STATUS" = "Accepted" ]; then
    # Right after acceptance the ticket may not have reached the CDN yet
    # (stapler Error 65, "Record not found"): retry instead of throwing the
    # accepted submission away and notarizing all over again.
    echo "==> Stapling"
    STAPLED=0
    for wait in 0 10 20 40 60; do
      sleep "$wait"
      if xcrun stapler staple "$APP"; then STAPLED=1; break; fi
      echo "   stapling failed; retrying in a moment" >&2
    done
    if [ "$STAPLED" = 0 ]; then
      echo "!! Notarization was ACCEPTED (submission id: $SUBMISSION_ID) but stapling kept failing." >&2
      echo "   Later, staple and package by hand:" >&2
      echo "     xcrun stapler staple $APP && ditto -c -k --norsrc --noextattr --keepParent $APP $ZIP" >&2
      rm -rf "$NOTARY_DIR"
      exit 1
    fi
    xcrun stapler validate "$APP"
    NOTARIZED=1
  else
    echo "" >&2
    echo "!! NOTARIZATION FAILED (status: ${STATUS:-none})" >&2
    if [ -n "$SUBMISSION_ID" ]; then
      echo "   submission id: $SUBMISSION_ID" >&2
      echo "   inspect with:  xcrun notarytool log $SUBMISSION_ID --keychain-profile $NOTARY_PROFILE" >&2
    else
      echo "   No submission was made; check the profile with:" >&2
      echo "     xcrun notarytool history --keychain-profile $NOTARY_PROFILE" >&2
    fi
    echo "" >&2
  fi
  rm -rf "$NOTARY_DIR"
fi

# ── The gate ─────────────────────────────────────────────────────────────────
# A signed but unnotarized zip looks exactly like a release and is refused on
# every Mac but this one, so it is not emitted unless asked for by name.
# dist/Navi.app stays built either way. Ad-hoc builds already said what they are.
if [ "$SIGNED_RELEASE" = 1 ] && [ "$NOTARIZED" = 0 ]; then
  if [ "${NAVI_ALLOW_UNNOTARIZED:-0}" = 1 ]; then
    echo "warn: PACKAGING AN UNNOTARIZED RELEASE (NAVI_ALLOW_UNNOTARIZED=1)."
    echo "      This zip will be refused by Gatekeeper on every Mac but this one."
  else
    echo "!! Refusing to package. dist/Navi.app is built but not notarized." >&2
    echo "   To package anyway: NAVI_ALLOW_UNNOTARIZED=1 $0" >&2
    exit 1
  fi
fi

# What a downloader's Mac will actually decide.
if [ "$NOTARIZED" = 1 ]; then
  SPCTL="$(spctl -a -t exec -vv "$APP" 2>&1 || true)"
  echo "$SPCTL" | sed 's/^/    /'
  case "$SPCTL" in
    *accepted*"source=Notarized Developer ID"*) ;;
    *) echo "error: Gatekeeper does not accept the stapled app as Notarized Developer ID" >&2
       exit 1 ;;
  esac
fi

# Bump bundle mtime so Icon Services notices a fresh app and reloads the icon.
# (Touching does not change the signature: mtimes are not sealed.)
touch "$APP" "$APP/Contents/Info.plist"

# ── Package ──────────────────────────────────────────────────────────────────
echo "==> Packaging $ZIP"
rm -f "$ZIP"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$ZIP"

du -h "$BIN" "$APP/Contents/MacOS/navi" "$ZIP" | sed 's/^/    /'
if [ "$NOTARIZED" = 1 ]; then
  echo "==> Done: signed, notarized and stapled."
  # Releases live on Codeberg (the only remote); there is no gh equivalent.
  echo "==> Attach $ZIP to the v${VERSION} release: https://codeberg.org/ganten1998/navi/releases"
else
  echo "==> Done (NOT notarized). Not for release: Gatekeeper refuses it on other Macs."
fi
