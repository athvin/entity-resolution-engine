# Path classification for `rust.yml`'s `detect` job. Sourced, not executed.
#
# `detect` answers three questions about a PR's changed paths, and both the
# `filter` step and the content-addressed `key` step need the same answers. They
# used to carry two copies of one regex with a comment asking future editors to
# keep them identical; this file is that comment made mechanical.
#
# DETECT_EXEMPT is a DENYLIST of paths the backend gate provably never reads.
# Adding an entry is removing something from the gate's sight, so only add a
# path nothing in `make check` reads. Enumerating rather than exempting a
# directory and listing exceptions is the FAIL-SAFE direction: something added
# later runs the gate until someone argues it should not, instead of skipping it
# until someone notices.
#
# `docs/` and `CLAUDE.md` are exempt as prefixes, but four files beneath them
# ARE gate inputs. They are re-admitted by DETECT_ERD and DETECT_DRIFT rather
# than by punching holes in DETECT_EXEMPT, because they do not all belong to the
# same job — see rust.yml's header.
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

# Everything the Rust gate never reads.
DETECT_EXEMPT='^(\.claude/|\.devcontainer/|docs/|CLAUDE\.md$|infra/terraform/|scripts/ci/tf-|scripts/hooks/|\.github/aw/|\.github/workflows/((infra|drift|devcontainer)\.yml$|shared/|agentics-maintenance\.yml$|[A-Za-z0-9._-]+\.md$|[A-Za-z0-9._-]+\.lock\.yml$))'

# A generated artifact `er-entity verify` re-renders and byte-compares.
DETECT_ERD='^docs/erd\.md$'

# Inputs to, and targets of, skills-check's generated references. A change here
# can make a committed generated block wrong, and the drift check that catches
# it rides the required `lint` job.
DETECT_DRIFT='^(docs/entity-framework-design\.md|docs/agent-skills\.md|CLAUDE\.md)$'

# The union: exempt by prefix, but still read by some gate, so the
# content-addressed pass key must keep hashing them or a change to one could
# ride a stale pass marker.
DETECT_COUPLED='^(docs/erd\.md|docs/entity-framework-design\.md|docs/agent-skills\.md|CLAUDE\.md)$'

# Classify a newline-separated list of changed paths on stdin. Emits the three
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

	noncode="$(printf '%s\n' "$changed" | grep -cvE "$DETECT_EXEMPT" || true)"
	erd_n="$(printf '%s\n' "$changed" | grep -cE "$DETECT_ERD" || true)"
	drift_n="$(printf '%s\n' "$changed" | grep -cE "$DETECT_DRIFT" || true)"

	if [ "${noncode:-0}" -gt 0 ]; then
		# Real code changed, so every job runs anyway. Forcing the two narrow
		# outputs true keeps downstream `if:` expressions from having to know
		# that `code` subsumes them.
		printf 'code=true\nerd=true\ndrift=true\n'
		return
	fi

	printf 'code=false\n'
	[ "${erd_n:-0}" -gt 0 ] && printf 'erd=true\n' || printf 'erd=false\n'
	[ "${drift_n:-0}" -gt 0 ] && printf 'drift=true\n' || printf 'drift=false\n'
}
