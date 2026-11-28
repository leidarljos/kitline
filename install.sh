#!/bin/sh
# Install kitline from the latest GitHub release.
#   curl -fsSL https://github.com/leidarljos/kitline/releases/latest/download/install.sh | sh
# The binary is written to $HOME/.local/bin, or to KITLINE_INSTALL_DIR.
# KITLINE_RELEASE_URL replaces the download base (http, https, or file).
set -eu

# Keep this line identical to channel in rust-toolchain.toml.
PIN=1.89.0
REPO=https://github.com/leidarljos/kitline
BASE=${KITLINE_RELEASE_URL:-https://github.com/leidarljos/kitline/releases/latest/download}

die() {
  printf '%s\n' "$1" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

fetch() {
  url=$1
  dest=$2
  if have curl; then
    curl -fsSL -o "$dest" "$url"
    return $?
  fi
  if have wget; then
    wget -q -O "$dest" "$url"
    return $?
  fi
  printf '%s\n' "curl or wget is required to download $url" >&2
  return 127
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
    return 0
  fi
  if have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
    return 0
  fi
  return 1
}

install_bin() {
  src=$1
  if [ -n "${KITLINE_INSTALL_DIR:-}" ]; then
    dest=$KITLINE_INSTALL_DIR
  else
    if [ -z "${HOME:-}" ]; then
      die "HOME is not set; set KITLINE_INSTALL_DIR"
    fi
    dest=$HOME/.local/bin
  fi
  mkdir -p "$dest" || die "cannot create $dest"
  cp "$src" "$dest/kitline" || die "cannot copy kitline to $dest/kitline"
  chmod +x "$dest/kitline" || die "cannot mark $dest/kitline executable"
  "$dest/kitline" --version || die "kitline --version failed"
}

# Source build only when rustup is already on PATH. Exits.
from_source() {
  reason=$1
  if ! have rustup; then
    die "$reason"
  fi
  printf '%s\n' "$reason" >&2
  printf '%s\n' "installing toolchain $PIN, then cargo install --locked --git $REPO" >&2
  rustup toolchain install "$PIN" || die "rustup toolchain install $PIN failed"
  root=$tmpdir/cargo-root
  cargo install --locked --git "$REPO" --root "$root" \
    || die "cargo install --locked --git $REPO failed"
  if [ ! -x "$root/bin/kitline" ]; then
    die "cargo install did not write $root/bin/kitline"
  fi
  install_bin "$root/bin/kitline"
  exit 0
}

# Download failed, or this machine has no published target.
give_up_or_source() {
  reason=$1
  if ! have rustup; then
    die "$reason"
  fi
  from_source "$reason"
}

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Darwin-arm64|Darwin-aarch64) target=aarch64-apple-darwin ;;
  *) target= ;;
esac

tmpdir=$(mktemp -d) || die "mktemp failed"
trap 'rm -rf "$tmpdir"' EXIT

if [ -z "$target" ]; then
  give_up_or_source "no prebuilt kitline for $(uname -s) $(uname -m)"
fi

if ! have curl && ! have wget; then
  give_up_or_source "curl or wget is required"
fi
if ! have sha256sum && ! have shasum; then
  give_up_or_source "sha256sum or shasum is required"
fi

asset=kitline-$target
sums=$tmpdir/SHA256SUMS
bin=$tmpdir/$asset

if ! fetch "$BASE/SHA256SUMS" "$sums"; then
  give_up_or_source "download failed: $BASE/SHA256SUMS"
fi
if ! fetch "$BASE/$asset" "$bin"; then
  give_up_or_source "download failed: $BASE/$asset"
fi

expected=$(awk -v name="$asset" '{ gsub(/\r/, ""); if ($2 == name) { print $1; exit } }' "$sums") \
  || die "could not read SHA256SUMS"
if [ -z "$expected" ]; then
  die "SHA256SUMS has no entry for $asset"
fi
case "$expected" in
  *[!0-9A-Fa-f]*) die "SHA256SUMS entry for $asset is not a sha256" ;;
esac
if [ "${#expected}" -ne 64 ]; then
  die "SHA256SUMS entry for $asset is not a sha256"
fi
actual=$(sha256_of "$bin") || die "sha256sum or shasum is required"
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
if [ "$expected" != "$actual" ]; then
  die "checksum mismatch for $asset"
fi

install_bin "$bin"
