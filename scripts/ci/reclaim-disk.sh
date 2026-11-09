#!/usr/bin/env bash
# Free the preinstalled runner-image trees this repo never reads, so a large
# `target/` and an in-job docker build have somewhere to go.
#
# WHAT GOES WRONG WITHOUT IT. `cargo test --workspace` links a separate debug
# binary per crate, and with bundled duckdb and full debug info that exceeds
# ubuntu-latest's ~14 GB of free space. The link step then dies with "No space
# left on device" — errno 28 — which surfaces as `linking with cc failed`, or
# as a failure in libduckdb's `ar` step, and reads as a toolchain error. That
# misdirection is the whole reason this step exists as a named step rather
# than as a tweak to a build profile.
#
# HEADROOM, NOT THE FIX. Measured: this takes the runner from 86 GB to 111 GB
# available — and `cargo test --workspace` still exhausted it, which is what
# sent the search to the debug-info settings now in `rust/Cargo.toml`'s
# `[profile.dev.package."*"]`. Keep both: the build is large even without C++
# symbols.
#
# The `df` before and after is part of the point, not decoration: a step that
# silently freed nothing looks identical in the log to one that worked.
#
# WHAT IS SAFE TO DELETE, and why. The dotnet, Android, GHC, boost and swift
# SDKs are read by nothing in this repo and return ~25 GB.
# `/opt/hostedtoolcache` is safe only because every caller installs its tools
# by other means — Rust via rustup, mold via apt, kind/kubectl/helm from
# pinned tarballs — and the actions they do use (sccache, rust-cache) are JS,
# so nothing reads `setup-*`'s cache. A STEP ADDED BEFORE A CALLER'S INVOCATION
# THAT POPULATES THE TOOLCACHE WOULD HAVE ITS INSTALL THROWN AWAY; add it
# after.
#
# A script rather than a third-party action, for the reason every `uses:` in
# this repo is SHA-pinned (rust.yml's `workflow pins (S9)` job enforces it):
# this needs `sudo rm -rf` on the runner and does not need a supply chain.
#
# Four jobs carried an identical copy — rust.yml's `test-shard`, `security`
# and `ducklake-e2e`, plus kind.yml's release-image build — each with its own
# re-derivation of the safety argument above. One copy now; the call sites
# keep only what is specific to them.
#
# NOT `set -e`: a runner image that has already dropped one of these paths
# must not fail the job it was called from. `rm -rf` is idempotent anyway.
#
# Run: bash scripts/ci/reclaim-disk.sh
set -euo pipefail

# Test jobs currently consume ~5-10 GiB, with ~88 GiB initially free. Keep
# 40 GiB for cold builds; image builders can request a larger reserve.
minimum="${CI_MIN_FREE_GIB:-40}"
case "$minimum" in ''|*[!0-9]*) echo "invalid CI_MIN_FREE_GIB" >&2; exit 2 ;; esac
available="$(df -Pk / | awk 'END {print $4}')"
df -h / | tail -1
if [ "$available" -ge "$((minimum * 1024 * 1024))" ]; then
    echo "disk cleanup skipped: at least $minimum GiB available"
    exit 0
fi
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc \
    /usr/local/share/boost /usr/share/swift
# Preserve hostedtoolcache: other steps may already have restored tools there.
df -h / | tail -1
available="$(df -Pk / | awk 'END {print $4}')"
if [ "$available" -lt "$((minimum * 1024 * 1024))" ]; then
    echo "::error::less than $minimum GiB available after cleanup"
    exit 1
fi
