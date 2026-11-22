#!/usr/bin/env bash
# Install `bca` (big-code-analysis) at a pinned version, verified against a
# digest committed here.
#
# Why a committed digest and not the release's own SHA256SUMS: a SHA256SUMS
# fetched from the same release proves only that the tarball matches what that
# release currently says, which is no help if the release itself changes. The
# digest below is the pin, the same way this repo pins actions by sha.
#
# Why a release binary and not `cargo install`: the tarball is 16 MB and
# extracts in seconds, where a build compiles twenty-odd tree-sitter grammars.
# Nothing we ship depends on this tool — it measures our source, it is not part
# of it — so it stays out of Cargo.lock entirely.
#
# Bumping: change VERSION, then replace all three digests from
# https://github.com/dekobon/big-code-analysis/releases/tag/v<VERSION>
# (SHA256SUMS), in one commit, and say in the message why the bump.
#
#   scripts/ci/install-bca.sh [destination-dir]    # default: .tools/bin
#
# Prints the absolute path of the installed binary on stdout.

set -euo pipefail

VERSION="2.6.0"
DEST="${1:-.tools/bin}"

case "$(uname -s)-$(uname -m)" in
Darwin-arm64) TARGET="aarch64-apple-darwin" ;;
Linux-aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
Linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
*)
	echo "install-bca: no pinned build for $(uname -s)-$(uname -m)." >&2
	echo "Add its digest from the release, or install bca yourself." >&2
	exit 1
	;;
esac

case "$TARGET" in
aarch64-apple-darwin) DIGEST="b01b965944ff4fefcf3a6a3e1e000a885198610fdf073d14bbeb07c8a4cbda6a" ;;
aarch64-unknown-linux-gnu) DIGEST="f3c1279539a9f99f0889743e222035cf3c03f131c228ba05d9c1626fc3080547" ;;
x86_64-unknown-linux-gnu) DIGEST="53df050fad1e9bc9081fbe89bb216db7631633d4a741f5ea2e9c322ae9cc0864" ;;
esac

mkdir -p "$DEST"
BIN="$(cd "$DEST" && pwd)/bca"

# Already the pinned version? Nothing to do. `bca --version` prints "bca <ver>"
# on its first line.
if [ -x "$BIN" ] && [ "$("$BIN" --version 2>/dev/null | head -1)" = "bca $VERSION" ]; then
	echo "$BIN"
	exit 0
fi

TARBALL="big-code-analysis-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/dekobon/big-code-analysis/releases/download/v${VERSION}/${TARBALL}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

curl -fsSL --retry 3 -o "$WORK/$TARBALL" "$URL"

# shasum on macOS, sha256sum on the runners.
if command -v sha256sum >/dev/null 2>&1; then
	ACTUAL="$(sha256sum "$WORK/$TARBALL" | cut -d' ' -f1)"
else
	ACTUAL="$(shasum -a 256 "$WORK/$TARBALL" | cut -d' ' -f1)"
fi

if [ "$ACTUAL" != "$DIGEST" ]; then
	echo "install-bca: digest mismatch for $TARBALL" >&2
	echo "  expected $DIGEST" >&2
	echo "  actual   $ACTUAL" >&2
	echo "Do not bypass this. Either the release was replaced or the pin is stale." >&2
	exit 1
fi

tar xzf "$WORK/$TARBALL" -C "$WORK"
install -m 0755 "$WORK/big-code-analysis-${VERSION}-${TARGET}/bca" "$BIN"
echo "$BIN"
