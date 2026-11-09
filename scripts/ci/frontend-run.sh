#!/usr/bin/env bash
# Invoked from frontend/ by CI. Keep the existing required job names while
# selecting public-site coverage only for a proven marketing-only diff.
set -euo pipefail

tier="${1:?expected check or e2e}"
shift
case "$tier" in
  check)
    if [ "${MARKETING_ONLY:-false}" = true ]; then
      pnpm lint
      pnpm format:check
      pnpm typecheck
    else
      pnpm check
      pnpm gen:api
      git diff --exit-code -- src/lib/api/schema.d.ts
    fi
    ;;
  e2e)
    if [ "${MARKETING_ONLY:-false}" = true ]; then
      # Playwright matches --grep against file + describe + test title.
      # Keep every marketing spec and the public landing entry in the axe
      # sweep. Never --pass-with-no-tests: an empty selection must fail.
      pnpm test:e2e '(^|/)(marketing(-[a-z0-9-]+)?|a11y)\.spec\.ts$' \
        --grep 'marketing(-[a-z0-9-]+)?\.spec\.ts|accessibility.*landing' "$@"
    else
      pnpm test:e2e "$@"
    fi
    ;;
  *) echo "unknown frontend tier: $tier" >&2; exit 2 ;;
esac
