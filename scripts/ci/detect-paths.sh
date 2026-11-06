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

# Everything the Rust gate never reads.
DETECT_EXEMPT='^(\.claude/|docs/|CLAUDE\.md$|infra/terraform/|scripts/ci/tf-|scripts/hooks/|\.github/aw/|\.github/workflows/((infra|drift)\.yml$|shared/|agentics-maintenance\.yml$|[A-Za-z0-9._-]+\.md$|[A-Za-z0-9._-]+\.lock\.yml$))'

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
