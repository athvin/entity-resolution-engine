#!/usr/bin/env bash
# The image name lives in two files that cannot read each other. Keep them equal.
#
#   .github/workflows/devcontainer.yml   IMAGE + IMAGE_TAG — what is PUSHED
#   .devcontainer/devcontainer.json      build.cacheFrom   — what is PULLED
#
# WHY THIS NEEDS A GATE AT ALL. Every other way these could drift announces
# itself; this one does not. If `cacheFrom` named a tag nothing publishes, the
# devcontainer build would simply find no cache and build from scratch — which
# SUCCEEDS. The workflow stays green, the acceptance criterion "a cold Reopen in
# Container pulls the prebuild" quietly stops holding, and the only symptom is
# that opening the container takes forty minutes again, months later, with
# nothing in any log to attribute it to. That is the exact failure #584 exists
# to prevent, arriving through the back door.
#
# Derived from the files rather than passed in through `env:`, so it is
# runnable on a laptop: `bash scripts/ci/devcontainer-image-check.sh`.
set -uo pipefail

cd "$(dirname "$0")/../.."

WORKFLOW=.github/workflows/devcontainer.yml
CONFIG=.devcontainer/devcontainer.json

for f in "$WORKFLOW" "$CONFIG"; do
	if [ ! -f "$f" ]; then
		echo "devcontainer-image-check: $f is missing — wrong directory?" >&2
		exit 2
	fi
done

# The workflow's top-level `env:` values. Anchored to two-space indentation so a
# job-level `IMAGE:` could not be picked up instead.
image=$(sed -n 's/^  IMAGE:[[:space:]]*\(.*\)$/\1/p' "$WORKFLOW" | head -n 1)
tag=$(sed -n 's/^  IMAGE_TAG:[[:space:]]*\(.*\)$/\1/p' "$WORKFLOW" | head -n 1)

# devcontainer.json is JSONC — it carries `//` comments, so no JSON parser will
# read it without stripping them first. One `sed` for the one field wanted is
# both smaller and less likely to be wrong than a comment stripper.
cache=$(sed -n 's/.*"cacheFrom"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$CONFIG" | head -n 1)

# POSITIVE CONTROL: a check that found none of its three inputs would report
# "they match" forever. Same zero-matches-is-failure rule as actionlint.sh and
# scripts-home-check.sh — and the same one this repo found actionlint.sh itself
# silently failing.
if [ -z "$image" ] || [ -z "$tag" ] || [ -z "$cache" ]; then
	echo "devcontainer-image-check: could not read all three values — the patterns are broken, not the tree." >&2
	echo "  IMAGE     from $WORKFLOW: '${image}'" >&2
	echo "  IMAGE_TAG from $WORKFLOW: '${tag}'" >&2
	echo "  cacheFrom from $CONFIG:   '${cache}'" >&2
	exit 2
fi

want="${image}:${tag}"
if [ "$cache" != "$want" ]; then
	echo "devcontainer-image-check: the prebuild would never be pulled." >&2
	echo "  $WORKFLOW pushes:  $want" >&2
	echo "  $CONFIG pulls:     $cache" >&2
	echo "devcontainer-image-check: make build.cacheFrom equal to IMAGE:IMAGE_TAG." >&2
	exit 1
fi

echo "devcontainer-image-check: ok ($want, pushed and pulled)"
