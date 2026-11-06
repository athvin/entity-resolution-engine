#!/usr/bin/env bash
# Tracked shell scripts live under scripts/ — fail naming any that do not.
#
# The scripts-consolidation program moved every stray (`frontend/dev/*.sh`,
# `.claude/hooks/*.sh`) under scripts/, so shell is reused instead of
# rewritten; this gate keeps it that way. A script is anything `git ls-files`
# tracks that either ends in `.sh` or whose first line is a sh/bash/zsh
# shebang — the shebang half is what catches an extensionless hook or helper.
#
# Content is only inspected on line one, deliberately: docs and workflows
# embed `#!/usr/bin/env bash` in code fences and heredocs, so a whole-file
# grep would flag prose.
#
# `grep -c`, never `grep -q`: `grep -q` closes the pipe on its first match,
# which SIGPIPEs the writer under `pipefail` — a found match reporting as
# missing, intermittently. See the header of scripts/ci/detect-paths.sh.
#
# Run: bash scripts/ci/scripts-home-check.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

# Positive control before asserting anything: a run that can see no shell
# scripts at all is in the wrong directory, not a clean tree — the same
# zero-matches-is-failure rule actionlint.sh applies to `uses:` pins.
known=$(git ls-files 'scripts/*.sh' | grep -c . || true)
if [ "${known:-0}" -eq 0 ]; then
	echo "scripts-home-check: found no tracked *.sh under scripts/ — wrong directory?" >&2
	exit 2
fi

fail=0

# Every tracked *.sh outside scripts/.
while IFS= read -r f; do
	[ -n "$f" ] || continue
	echo "scripts-home-check: $f — tracked shell scripts live under scripts/ (CLAUDE.md, conventions)" >&2
	fail=1
done < <(git ls-files '*.sh' ':!scripts/')

# Every tracked extensionless shell script outside scripts/: shebang on the
# FIRST line only, bounded so a binary's newline-free start cannot balloon it.
while IFS= read -r f; do
	[ -n "$f" ] && [ -f "$f" ] || continue
	first=$(head -c 256 -- "$f" 2>/dev/null | head -n 1)
	case $first in
	'#!'*) ;;
	*) continue ;;
	esac
	hit=$(printf '%s\n' "$first" | grep -cE '^#!.*[/[:space:]](ba|z)?sh([[:space:]]|$)' || true)
	if [ "${hit:-0}" -gt 0 ]; then
		echo "scripts-home-check: $f — shell-shebang file outside scripts/ (CLAUDE.md, conventions)" >&2
		fail=1
	fi
done < <(git ls-files ':!scripts/' ':!*.sh')

if [ "$fail" -ne 0 ]; then
	echo "scripts-home-check: move the files above under scripts/ (scripts/ci/, scripts/frontend-dev/, scripts/hooks/, …)" >&2
	exit 1
fi

echo "scripts-home-check: ok (${known} scripts under scripts/, none astray)"
