#!/usr/bin/env bash
# Cross-build Navi release artifacts for Linux and Windows from a macOS host.
#
#   scripts/build-cross.sh          # builds + packages everything into dist/
#
# Requirements (all via Homebrew/cargo):
#   brew install zig llvm
#   cargo install cargo-zigbuild cargo-xwin
#   rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-msvc
#
# Note: if Homebrew's rust is also installed, its cargo/rustc shadow rustup's
# on PATH and cross-target std libs won't be found; hence the PATH prefix.
# cargo-xwin builds bundled C (sqlite) with llvm-lib / clang-cl / lld-link,
# which Homebrew's llvm keeps off PATH (keg-only); the script finds the keg.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep '^version' navi/Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
GLIBC="2.32"   # floor for released Linux binaries

TOOLCHAIN_BIN="$(rustup which cargo 2>/dev/null | xargs dirname || true)"
[ -n "$TOOLCHAIN_BIN" ] && export PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"

# llvm-lib for the Windows step: PATH, else Homebrew's llvm keg (llvm, or the
# newest versioned llvm@N). Checked up front so a missing LLVM fails here,
# not after both Linux tarballs.
if ! command -v llvm-lib >/dev/null 2>&1; then
  BREW="$(brew --prefix 2>/dev/null || echo /opt/homebrew)"
  for keg in "$BREW/opt/llvm" $(ls -d "$BREW"/opt/llvm@* 2>/dev/null | sort -t@ -k2 -n -r); do
    if [ -x "$keg/bin/llvm-lib" ]; then export PATH="$keg/bin:$PATH"; break; fi
  done
fi
command -v llvm-lib >/dev/null 2>&1 || { echo "error: llvm-lib not found (brew install llvm); the Windows build needs it" >&2; exit 1; }

# Cargo cannot link build scripts inside Dropbox ("Operation not permitted"),
# so the build goes where build-macos.sh puts it; the artifacts are copied
# from there (never from a stale repo-local target/).
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  case "$ROOT" in
    *Dropbox*) export CARGO_TARGET_DIR="$HOME/.cache/navi-target" ;;
    *)         export CARGO_TARGET_DIR="$ROOT/target" ;;
  esac
fi
TARGET_DIR="$CARGO_TARGET_DIR"
echo "==> target-dir=$TARGET_DIR"

mkdir -p dist

package_linux() { # $1 = rust target, $2 = artifact arch name
  local target="$1" arch="$2"
  local stage="dist/navi-${VERSION}-linux-${arch}"
  cargo zigbuild --release --target "${target}.${GLIBC}" -p navi
  rm -rf "$stage"; mkdir -p "$stage"
  cp "$TARGET_DIR/${target}/release/navi" "$stage/"
  cp assets/navi.desktop "$stage/"
  cp assets/icon.png "$stage/"
  # dist/ is in Dropbox: the files carry com.apple.provenance and Dropbox
  # xattrs, which macOS tar would ship as AppleDouble ._ files
  COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C dist -czf "${stage}.tar.gz" "navi-${VERSION}-linux-${arch}"
  rm -rf "$stage"
  echo "==> ${stage}.tar.gz"
}

echo "==> Navi $VERSION: Linux x86_64"
package_linux x86_64-unknown-linux-gnu x86_64

echo "==> Navi $VERSION: Linux aarch64"
package_linux aarch64-unknown-linux-gnu aarch64

echo "==> Navi $VERSION: Windows x86_64"
cargo xwin build --release --target x86_64-pc-windows-msvc -p navi
rm -f "dist/navi-${VERSION}-windows-x86_64.zip"
(cd "$TARGET_DIR/x86_64-pc-windows-msvc/release" && zip -q "$ROOT/dist/navi-${VERSION}-windows-x86_64.zip" navi.exe)
echo "==> dist/navi-${VERSION}-windows-x86_64.zip"

ls -lh dist/navi-${VERSION}-* 2>/dev/null | sed 's/^/    /'
