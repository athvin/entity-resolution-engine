#!/usr/bin/env bash
# Install actionlint from its pinned release tarball. THE PIN LIVES HERE.
#
# WHY THIS EXISTS. The S9 job used to install actionlint with
# `taiki-e/install-action` and `continue-on-error: true`. That install has
# NEVER succeeded: the action resolves an unknown tool as a Rust crate, and
# actionlint is a Go binary that is not on crates.io, so every run logged
# `× For crate actionlint: actionlint is not found`, the swallowed failure left
# the binary absent, and `actionlint.sh` skipped its linting half and exited 0.
# The job's green came entirely from the pin check. Measured on run
# 42410745827, which is the post-merge `rust` run on `main`.
#
# It is not that the action declines non-Rust tools — it ships `shellcheck`,
# `shfmt` and `protoc`. actionlint simply is not in its `TOOLS.md`, so no
# version of that step could have worked.
#
# A PINNED TARBALL RATHER THAN A THIRD-PARTY ACTION, for the reason
# `install-helm.sh` gives: every `uses:` here is SHA-pinned (the very job this
# installs for enforces it), and a pinned download needs no SHA to stay
# current.
#
# WHY THIS ONE ALSO VERIFIES A CHECKSUM, where install-helm.sh does not:
# actionlint publishes an official `actionlint_<v>_checksums.txt` with every
# release, so verification costs one fetch and one comparison. A pin fixes
# WHICH bytes you asked for; a checksum is what tells you that is what you got.
# (install-helm.sh pipes curl straight into `sudo tar` with no checksum. Larger
# blast radius, outside this change's scope — flagged, not fixed here.)
#
# BUMPING: change ACTIONLINT_VERSION and nothing else. The checksum is fetched
# from that release, so the two cannot drift apart — there is no second copy to
# forget. Run it afterwards: a newer actionlint can add rules, and this gate is
# mandatory now, so a bump is a change that has to be observed green.
#
# Run: bash scripts/ci/install-actionlint.sh [dest-dir]   # default /usr/local/bin
# Exit: 0 installed and self-reported · 1 download, checksum or extraction failed
set -euo pipefail

ACTIONLINT_VERSION="${ACTIONLINT_VERSION:-1.7.12}"
DEST="${1:-/usr/local/bin}"

die() {
	printf 'install-actionlint: %s\n' "$1" >&2
	exit 1
}

# uname -m spells the same architectures differently from Go's release names.
case "$(uname -s)" in
Linux) os=linux ;;
Darwin) os=darwin ;;
*) die "unsupported OS $(uname -s); actionlint publishes linux, darwin, freebsd and windows" ;;
esac
case "$(uname -m)" in
x86_64 | amd64) arch=amd64 ;;
arm64 | aarch64) arch=arm64 ;;
*) die "unsupported architecture $(uname -m)" ;;
esac

tarball="actionlint_${ACTIONLINT_VERSION}_${os}_${arch}.tar.gz"
base="https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$tarball" "$base/$tarball" \
	|| die "could not download $base/$tarball"
curl -fsSL -o "$tmp/checksums.txt" "$base/actionlint_${ACTIONLINT_VERSION}_checksums.txt" \
	|| die "could not download the checksums file for v${ACTIONLINT_VERSION}"

want="$(awk -v f="$tarball" '$2 == f { print $1 }' "$tmp/checksums.txt")"
# An empty `want` must not compare equal to an empty `got`: that is how a
# checksum step silently verifies nothing when the asset name changes.
[ -n "$want" ] || die "$tarball is not listed in the v${ACTIONLINT_VERSION} checksums file"

if command -v sha256sum >/dev/null 2>&1; then
	got="$(sha256sum "$tmp/$tarball" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
	got="$(shasum -a 256 "$tmp/$tarball" | cut -d' ' -f1)"
else
	die "neither sha256sum nor shasum is available; refusing to install unverified"
fi

if [ "$want" != "$got" ]; then
	printf 'install-actionlint: CHECKSUM MISMATCH for %s\n' "$tarball" >&2
	printf '  expected %s\n  got      %s\n' "$want" "$got" >&2
	die "refusing to install"
fi

tar xzf "$tmp/$tarball" -C "$tmp" actionlint || die "could not extract actionlint from $tarball"

# `sudo` only when the destination is not writable, so a local run into a
# user-owned directory needs no privilege at all.
if [ -w "$DEST" ]; then
	install -m 0755 "$tmp/actionlint" "$DEST/actionlint" || die "could not install into $DEST"
else
	sudo install -m 0755 "$tmp/actionlint" "$DEST/actionlint" || die "could not install into $DEST"
fi

# Self-report, for the reason install-helm.sh does: a missing binary should fail
# here and not three steps downstream as a confusing lint error.
printf 'install-actionlint: installed %s into %s (sha256 verified)\n' \
	"$("$DEST/actionlint" --version | head -1)" "$DEST"
