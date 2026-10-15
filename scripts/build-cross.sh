#!/usr/bin/env bash
# Cross-build Navi release artifacts for Linux, FreeBSD and Windows from a
# macOS host, then write dist/SHA256SUMS over every artifact of this version
# (the macOS zip too, so build it first with scripts/build-macos.sh).
#
#   scripts/build-cross.sh          # builds + packages everything into dist/
#
# Requirements (all via Homebrew/cargo):
#   brew install zig llvm
#   cargo install cargo-zigbuild cargo-xwin
#   rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-unknown-freebsd x86_64-pc-windows-msvc
#
# FreeBSD links only libc/libm/libthr (zig ships their stubs); X11, xkbcommon
# and GL are dlopened at run time, as on Linux, so no FreeBSD sysroot is needed.
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

package_unix() { # $1 = rust target, $2 = os name in the artifact, $3 = arch name, $4 = zigbuild target
  local target="$1" os="$2" arch="$3" zig_target="$4"
  local stage="dist/navi-${VERSION}-${os}-${arch}"
  cargo zigbuild --release --target "$zig_target" -p navi
  rm -rf "$stage"; mkdir -p "$stage"
  cp "$TARGET_DIR/${target}/release/navi" "$stage/"
  cp assets/navi.desktop "$stage/"
  cp assets/icon.png "$stage/"
  # dist/ is in Dropbox, whose files come out 0600: set the modes a system-wide
  # install needs, and ship no build-machine owner
  chmod 755 "$stage" "$stage/navi"; chmod 644 "$stage/navi.desktop" "$stage/icon.png"
  # dist/ is in Dropbox: the files carry com.apple.provenance and Dropbox
  # xattrs, which macOS tar would ship as AppleDouble ._ files
  COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs --uid 0 --gid 0 --uname root --gname root -C dist -czf "${stage}.tar.gz" "navi-${VERSION}-${os}-${arch}"
  rm -rf "$stage"
  echo "==> ${stage}.tar.gz"
}

echo "==> Navi $VERSION: Linux x86_64"
package_unix x86_64-unknown-linux-gnu linux x86_64 "x86_64-unknown-linux-gnu.${GLIBC}"

echo "==> Navi $VERSION: Linux aarch64"
package_unix aarch64-unknown-linux-gnu linux aarch64 "aarch64-unknown-linux-gnu.${GLIBC}"

echo "==> Navi $VERSION: FreeBSD x86_64"
package_unix x86_64-unknown-freebsd freebsd x86_64 x86_64-unknown-freebsd

echo "==> Navi $VERSION: Windows x86_64"
# static CRT: a clean Windows has no VCRUNTIME140.dll (the VC++ redistributable)
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static" \
  cargo xwin build --release --target x86_64-pc-windows-msvc -p navi
rm -f "dist/navi-${VERSION}-windows-x86_64.zip"
(cd "$TARGET_DIR/x86_64-pc-windows-msvc/release" && zip -q "$ROOT/dist/navi-${VERSION}-windows-x86_64.zip" navi.exe)
echo "==> dist/navi-${VERSION}-windows-x86_64.zip"

# Checksums over this version's release artifacts (never an -adhoc or
# -unnotarized macOS zip). org-roam-navi-install refuses a download that
# doesn't match its line here.
(
  cd dist
  shopt -s nullglob
  # one glob, then a case-sensitive test: APFS is case-insensitive, so
  # Navi-*.zip (macOS) would also match navi-*.zip (Windows)
  release=(); mac=0
  for f in *-"${VERSION}"-*.zip *-"${VERSION}"-*.tar.gz; do
    case "$f" in *-adhoc.zip|*-unnotarized.zip) continue ;; esac
    [[ "$f" == Navi-* ]] && mac=1
    release+=("$f")
  done
  [ "$mac" = 1 ] || echo "warn: no macOS zip for $VERSION in dist/ (run scripts/build-macos.sh first); SHA256SUMS won't list it" >&2
  shasum -a 256 "${release[@]}" > SHA256SUMS
)
echo "==> dist/SHA256SUMS"

ls -lh dist/*-${VERSION}-* dist/SHA256SUMS 2>/dev/null | sed 's/^/    /'
