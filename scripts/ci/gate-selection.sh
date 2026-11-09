#!/usr/bin/env bash
# A required aggregate must distinguish intentional skips from missing work.
# GATE_LEGS is name=expected,result (expected is true or false).
set -euo pipefail
[ "${DETECT_RESULT:-}" = success ] || { echo '::error::classification failed'; exit 1; }
[ -n "${GATE_LEGS:-}" ] || { echo '::error::no gate legs'; exit 1; }
while IFS='=,' read -r name expected result; do
    [ -n "$name" ] || continue
    case "$expected:$result" in
        true:success|false:skipped) echo "$name: $result (expected=$expected)" ;;
        *) echo "::error::$name: expected=$expected result=$result"; exit 1 ;;
    esac
done <<< "$GATE_LEGS"
