#!/usr/bin/env bash
# The per-job build-cost report: sccache's hit rate, how large `rust/target`
# grew, and what is left on the runner's root disk.
#
# NEVER A GATE. The call sites are `if: always()` so a red job still reports,
# and every line here is `|| true` so the reporting itself cannot fail one —
# it fires on the failing job too, and a diagnostic that can fail would mask
# the real error with its own. (`du` in particular exits non-zero on a
# `target/` a concurrent build is pruning under it.)
#
# WHY IT IS COLLECTED AT ALL. The sccache hit rate has never been observable
# in this repo, which is why nobody could say whether it earns the ~4.3 GiB
# and ~2,700 entries it takes out of a 10 GB per-repository cache. It is the
# input to the two decisions deferred in the `ci-and-supply-chain` skill's
# build-cost reference: whether to move sccache to S3, and whether it could be
# dropped (which is also precondition 1 for revisiting headstart).
#
# Read it as a pair: a cold run shows what the misses cost, a warm run shows
# what the hits save. One number alone says nothing.
#
# Seven steps in rust.yml carried this inline, in two variants — five printed
# `df` and two did not. The `df` line is the one that distinguishes "this
# build is slow" from "this runner is out of space", and a full disk surfaces
# as `linking with cc failed` rather than as ENOSPC, so the job that omits it
# is exactly the job whose failure will be misread. This always prints all
# three: the `du`/`df` pair is also the input for deciding whether a leg can
# drop its disk-reclaim step, and a job that never reclaimed still benefits
# from knowing how close it came.
#
# Run: bash scripts/ci/build-cost.sh  (from the repo root; the script cds)
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 0

sccache --show-stats || true
du -sh rust/target || true
df -h / | tail -1 || true
