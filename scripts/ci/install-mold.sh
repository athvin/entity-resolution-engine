#!/usr/bin/env bash
# Install mold, retried and time-boxed — one copy for every CI job that links
# with it (rust.yml uses it seven times; agent-task.yml carries the eighth).
#
# Retried and bounded because this is an unpinned network dependency on the
# critical path of the required jobs. Observed hanging for 20+ minutes on two
# consecutive attempts on 2026-10-22 — and on a required job a hang is not a
# failure, it is the 45-minute job timeout, which reads as a non-green check
# even though every gate passed. `timeout` bounds each attempt so a stuck
# mirror is retried rather than waited on; callers set `timeout-minutes: 13`
# on the step so all three attempts fit (3 x 2 x 120s = 12 min).
#
# On total failure this FALLS BACK rather than fails — mold is a speed
# optimisation, not a correctness requirement. The fallback strips mold from
# RUSTFLAGS via $GITHUB_ENV; see the RUSTFLAGS comment in rust.yml's `env:`
# for why that is safe and what it costs.
#
# Run (CI): bash scripts/ci/install-mold.sh
set -uo pipefail

for i in 1 2 3; do
	# Clear the package lists first, so a retry actually re-fetches.
	# Observed 2026-11-05: `apt-get update` reached ONLY
	# packages.microsoft.com and never contacted azure.archive.ubuntu.com,
	# exited 0 with a partial index, and `apt-get install -y mold` then
	# reported "Unable to locate package mold". All three attempts repeated
	# it, because each one Hit:1 the cache it had just been given rather
	# than refetching — so the retry loop could not recover from the one
	# state it exists for. mold IS on the image's archive
	# (2.40.4+dfsg-2.1ubuntu1): a different leg of the same run, on the same
	# ubuntu-26.04 image, installed it.
	sudo rm -rf /var/lib/apt/lists/*
	# ...and make a repository that could not be fetched a FAILURE rather
	# than a warning. Without this a partial update exits 0 and the `&&`
	# stops guarding anything: install runs against an index missing the
	# package it is about to look for, and the error surfaces as a confusing
	# "no such package" instead of "the mirror was unreachable".
	if timeout 120 sudo apt-get update -o APT::Update::Error-Mode=any \
		&& timeout 120 sudo apt-get install -y mold; then
		exit 0
	fi
	echo "mold install attempt $i failed or timed out; retrying"
	sleep 5
done

echo "::warning title=mold unavailable, using the default linker::three attempts failed (an apt mirror, not this diff); this job will be much slower and may hit its timeout"
echo "RUSTFLAGS=${RUSTFLAGS//-C link-arg=-fuse-ld=mold/}" >> "$GITHUB_ENV"
