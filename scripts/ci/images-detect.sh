#!/usr/bin/env bash
# images.yml's detect step: which trees changed, per image.
#
# Inputs (env): EVENT, BEFORE, SHA. Outputs ($GITHUB_OUTPUT): rust, frontend.
# Fail-safe direction: an uncomputable range builds both.
if [ "$EVENT" != "push" ] || [ -z "$BEFORE" ] \
  || [ "$BEFORE" = "0000000000000000000000000000000000000000" ] \
  || ! git cat-file -e "$BEFORE^{commit}" 2>/dev/null; then
  echo "rust=true" >> "$GITHUB_OUTPUT"
  echo "frontend=true" >> "$GITHUB_OUTPUT"
  exit 0
fi
changed="$(git diff --name-only "$BEFORE...$SHA")"
# `grep -c` (not -q) reads all input: -q closes the pipe early and can
# SIGPIPE `git diff` under pipefail, which reads as a false skip.
for tree in rust frontend; do
  n="$(printf '%s\n' "$changed" | grep -cE "^${tree}/" || true)"
  if [ "${n:-0}" -gt 0 ]; then
    echo "${tree}=true" >> "$GITHUB_OUTPUT"
  else
    echo "${tree}=false" >> "$GITHUB_OUTPUT"
  fi
done
# A change to this workflow rebuilds both.
if printf '%s\n' "$changed" | grep -qE '^\.github/workflows/images\.yml$'; then
  echo "rust=true" >> "$GITHUB_OUTPUT"
  echo "frontend=true" >> "$GITHUB_OUTPUT"
fi
