#!/usr/bin/env bash
# Stop the functional tier's two servers so the next run starts from source.
#
# `playwright.config.ts` sets `reuseExistingServer` outside CI, which adopts
# whatever is already listening rather than starting it. That is what makes an
# iteration loop fast, and it is also why a server left running from an earlier
# build keeps serving the old bundle — and why a mock-erserver left running
# ignores fixture edits in `tests/mock-erserver/`. Killing both is the remedy;
# the app's webServer command rebuilds on the way back up.
#
# Silent when nothing is listening: this runs as a precondition, not a check.
set -uo pipefail

for port in 3100 8010; do
    # `lsof -t` prints one pid per line and exits non-zero when the port is
    # free, which is not an error here.
    while read -r pid; do
        [[ -n "${pid}" ]] && kill "${pid}" 2>/dev/null
    done < <(lsof -ti "tcp:${port}" 2>/dev/null || true)
done

exit 0
