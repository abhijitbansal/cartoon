#!/bin/sh
# Install the prebuilt cartoon binary from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/abhijitbansal/cartoon/main/install.sh | sh
#
# Linux (x86_64, aarch64; static musl, any distro) and macOS (x86_64, arm64).
# The tarball is verified against the release's SHA256SUMS before install.
#
# Environment:
#   CARTOON_VERSION      release to install, e.g. 0.7.0 (default: latest)
#   CARTOON_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   CARTOON_BASE_URL     release download base (default: GitHub releases;
#                        tests point it at a local mirror)
set -eu

REPO="abhijitbansal/cartoon"

err() {
  echo "cartoon-install: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) err "unsupported OS $(uname -s); use \`npm install -g cartoon-wrap\` or \`cargo install cartoon\`" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) err "unsupported architecture $(uname -m); use \`cargo install cartoon\`" ;;
esac
target="${arch}-${os}"
asset="cartoon-${target}.tar.gz"

if [ -n "${CARTOON_BASE_URL:-}" ]; then
  base="$CARTOON_BASE_URL"
elif [ -n "${CARTOON_VERSION:-}" ]; then
  base="https://github.com/${REPO}/releases/download/v${CARTOON_VERSION#v}"
else
  base="https://github.com/${REPO}/releases/latest/download"
fi
dest="${CARTOON_INSTALL_DIR:-$HOME/.local/bin}"

fetch() { # url outfile
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https,file' -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    err "need curl or wget"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    err "need sha256sum or shasum to verify the download"
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "cartoon-install: downloading $asset"
fetch "$base/$asset" "$tmp/$asset" || err "download failed: $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || err "download failed: $base/SHA256SUMS"

expected="$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || err "$asset is not listed in SHA256SUMS"
actual="$(sha256 "$tmp/$asset")"
[ "$expected" = "$actual" ] || err "checksum mismatch for $asset (expected $expected, got $actual)"

tar -xzf "$tmp/$asset" -C "$tmp" cartoon
mkdir -p "$dest"
# Copy then rename, so a running cartoon is never overwritten in place.
cp "$tmp/cartoon" "$dest/.cartoon.new"
chmod 755 "$dest/.cartoon.new"
mv -f "$dest/.cartoon.new" "$dest/cartoon"

echo "cartoon-install: installed $("$dest/cartoon" --version 2>/dev/null || echo cartoon) to $dest/cartoon"
case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "cartoon-install: $dest is not on PATH; add: export PATH=\"$dest:\$PATH\"" ;;
esac
