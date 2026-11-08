# Path classification for `rust.yml`'s `detect` job. Sourced, not executed.
#
# `detect` answers four questions about a PR's changed paths, and both the
# `filter` step and the content-addressed `key` step need the same answers. They
# used to carry two copies of one regex with a comment asking future editors to
# keep them identical; this file is that comment made mechanical.
#
# Three of the four are "which jobs must run"; the fourth, `empty`, is "are
# there any changed paths at all" — see `detect_classify`, where the denylist
# direction read a zero-file PR as a code change for as long as this file has
# existed.
#
# DETECT_EXEMPT is a DENYLIST of paths the backend gate provably never reads.
# Adding an entry is removing something from the gate's sight, so only add a
# path nothing in `make check` reads. Enumerating rather than exempting a
# directory and listing exceptions is the FAIL-SAFE direction: something added
# later runs the gate until someone argues it should not, instead of skipping it
# until someone notices.
#
# `.claude/` and `CLAUDE.md` are exempt as prefixes, but four files beneath them
# ARE gate inputs. They are re-admitted by DETECT_ERD and DETECT_DRIFT rather
# than by punching holes in DETECT_EXEMPT, because they do not all belong to the
# same job — see rust.yml's header.
#
# There is no `docs/` entry any more: the directory is gone. The four gate
# inputs that used to live there are now skill references under `.claude/`,
# which this regex already exempts by prefix, so the re-admission mechanism is
# unchanged and only the paths moved. `docs/` is NOT left in the denylist as a
# courtesy to a future re-introduction — an exemption for a directory that does
# not exist is a hole nobody is watching, and the fail-safe direction is that a
# reintroduced `docs/` file runs the gate until someone argues it should not.
#
# Note what is NOT exempt and must not become so: this file. A change to the
# classifier itself has to run the gate it classifies for.
#
# THE WORKFLOW ENTRIES ARE NAMED FILE BY FILE, never as `^\.github/workflows/`,
# and that is the one part of this regex that needs care. `rust.yml`,
# `frontend.yml` and `images.yml` live in that directory too and must keep
# triggering the gates they define — otherwise a change to a gate would stop
# reporting on exactly the PR most likely to break it, which is the
# `paths:`-filter trap in CLAUDE.md wearing a different hat. What IS exempt:
# `infra.yml` and `drift.yml`, which define no Rust job and cannot affect the
# workspace build (an infra-only PR was paying the full ~40-minute backend
# gate — `test` plus the DuckLake e2e — to prove it), and the `gh aw` agentic
# set: the `.md` a human edits, the `.lock.yml` `gh aw compile` emits from it,
# the `shared/` imports they pull in, `agentics-maintenance.yml` which that
# compile also generates, and `.github/aw/` (gh-aw's own metadata, the
# resolved action SHAs). None of those shapes can capture a hand-written
# `.yml` workflow, so they cannot widen the exemption past what is argued
# for here.
#
# `.devcontainer/` and `devcontainer.yml` join that list for the same reason
# `infra.yml` did: they define no Rust job and cannot affect the workspace
# build. The development image is built BY `.github/workflows/devcontainer.yml`
# and read by nothing in `make check` — no crate, no manifest and no gate
# script references `.devcontainer/`.
#
# What keeps that exemption honest is that every gate which DOES read those two
# paths is ungated. `scripts-home-check.sh` would catch a stray
# `.devcontainer/*.sh`, `workflow-shell-check.sh` budgets `devcontainer.yml`'s
# `run:` blocks and `actionlint.sh` checks its pins — and all three run in
# rust.yml's `workflow pins (S9)` job, which is deliberately NOT gated on
# `detect`. So exempting these removes them from the compile tier's sight
# without removing them from anything that actually inspects them.
#
# Note what is NOT exempt and must not become so: `scripts/devcontainer-*.sh`
# and `scripts/ci/devcontainer-*.sh`. They live under `scripts/`, which this
# regex does not cover, so a change to one pays the full backend gate. That is
# the fail-safe direction and is left alone.

# `.agents/` and `AGENTS.md` are where Codex looks for the skills and the
# project instructions this repo authors under `.claude/skills/` and
# `CLAUDE.md`. Both are symlinks into the authored side — written once, bridged
# across, which `scripts/ci/agent-bridge-check.sh` enforces — so nothing in
# `make check` reads either, for the same reason `.claude/` and `CLAUDE.md`
# aren't read. What keeps the exemption honest is the same mechanism as
# `.devcontainer/` two paragraphs up: the gate that DOES read them runs ungated
# in rust.yml's `skills` job.
#
# Everything the Rust gate never reads.
DETECT_EXEMPT='^(\.claude/|\.agents/|\.devcontainer/|CLAUDE\.md$|AGENTS\.md$|infra/terraform/|scripts/ci/tf-|scripts/hooks/|\.github/aw/|\.github/workflows/((infra|drift|devcontainer)\.yml$|shared/|agentics-maintenance\.yml$|[A-Za-z0-9._-]+\.md$|[A-Za-z0-9._-]+\.lock\.yml$))'

# A generated artifact `er-entity verify` re-renders and byte-compares.
DETECT_ERD='^\.claude/skills/entity-framework/references/erd\.md$'

# Inputs to, and targets of, skills-check's generated references. A change here
# can make a committed generated block wrong, and the drift check that catches
# it rides the required `lint` job.
DETECT_DRIFT='^(\.claude/skills/entity-framework/references/entity-framework-design\.md|\.claude/skills/skill-writer/references/agent-skills\.md|CLAUDE\.md)$'

# The union: exempt by prefix, but still read by some gate, so the
# content-addressed pass key must keep hashing them or a change to one could
# ride a stale pass marker.
DETECT_COUPLED='^(\.claude/skills/entity-framework/references/erd\.md|\.claude/skills/entity-framework/references/entity-framework-design\.md|\.claude/skills/skill-writer/references/agent-skills\.md|CLAUDE\.md)$'

# Classify a newline-separated list of changed paths on stdin. Emits the four
# `key=value` lines `detect` writes to $GITHUB_OUTPUT, in that order.
#
# The workflow calls this and so does `detect-paths-test.sh`, which is the point:
# a test that re-implemented the branching could pass while the workflow did
# something else.
#
# `grep -c`, never `grep -q`: `grep -q` closes the pipe on its first match, which
# SIGPIPEs the writer and makes the pipeline exit 141 under `pipefail` — a found
# match reporting as missing, intermittently. The repo has been bitten by this in
# `skills-check.sh` and `tf-helpers-test.sh` already. `|| true` absorbs grep's
# exit 1 on zero matches under `set -e`.
detect_classify() {
	local changed noncode erd_n drift_n
	changed="$(cat)"

	# A PULL REQUEST THAT INTRODUCES NO FILES AT ALL. `base...head` empty means
	# head's tree equals the merge base, so there is provably nothing to
	# compile — but the DENYLIST direction could not see that on its own, and
	# ran the whole backend gate instead. The blank line below is why: the
	# counting pipelines re-expand `$changed` with `printf '%s\n'`, which for an
	# empty string emits ONE BLANK LINE, a blank line matches no exemption, and
	# so `grep -cv` reported one non-exempt path. Measured before this guard
	# existed: an empty diff classified `code=true erd=true drift=true`.
	#
	# It is the four ALLOWLIST detects — frontend, infra, images, devcontainer —
	# that never had the bug, because zero paths is zero matches there. Only
	# this one inverts.
	#
	# Not a rare accident, either: `claim-ticket` opens exactly this shape on
	# purpose — a draft PR pushed from an empty commit, so the claim is visible
	# on GitHub before any code exists — and every one of those paid ~10m across
	# the whole compile matrix plus its ~0.6 GB of sccache S3 egress to prove a
	# tree identical to `main`.
	#
	# The test is a plain `-z` rather than whitespace-stripping because
	# `changed="$(cat)"` has already dropped the trailing newline: an empty
	# stdin and a single-newline stdin both arrive here as the empty string.
	# `detect-paths-test.sh` reaches the case by passing no paths at all.
	#
	# `empty` is a SEPARATE output rather than folded into `code` because two
	# different jobs need the two answers: the compile tier skips (which branch
	# protection accepts), while `empty pull request` fails — so a claim PR
	# reads red rather than green with nothing proved.
	if [ -z "$changed" ]; then
		printf 'code=false\nerd=false\ndrift=false\nempty=true\n'
		return
	fi

	noncode="$(printf '%s\n' "$changed" | grep -cvE "$DETECT_EXEMPT" || true)"
	erd_n="$(printf '%s\n' "$changed" | grep -cE "$DETECT_ERD" || true)"
	drift_n="$(printf '%s\n' "$changed" | grep -cE "$DETECT_DRIFT" || true)"

	if [ "${noncode:-0}" -gt 0 ]; then
		# Real code changed, so every job runs anyway. Forcing the two narrow
		# outputs true keeps downstream `if:` expressions from having to know
		# that `code` subsumes them.
		printf 'code=true\nerd=true\ndrift=true\nempty=false\n'
		return
	fi

	printf 'code=false\n'
	[ "${erd_n:-0}" -gt 0 ] && printf 'erd=true\n' || printf 'erd=false\n'
	[ "${drift_n:-0}" -gt 0 ] && printf 'drift=true\n' || printf 'drift=false\n'
	# Reached only with at least one changed path, so never empty here.
	printf 'empty=false\n'
}
