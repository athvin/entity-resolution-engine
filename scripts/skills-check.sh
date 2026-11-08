#!/usr/bin/env bash
# The agent-skill gate. Two modes, split by what their output derives from —
# which is what makes the gate correct rather than decorative.
#
#   lint   Validates the skills themselves against the Agent Skills authoring
#          rules (frontmatter limits, body size, one-level references, resolving
#          links, an evals.md per skill). Pure shell: no cargo, no node, no
#          network. This mode MUST run on skills-only PRs, which is why CI puts
#          it in a job that does not gate on `detect`.
#
#   regen  Rewrites the GENERATED blocks inside skill reference files from the
#          source of truth (entities/, the crate tree, analyze.rs, the
#          workflows, the frontend config) and reports any drift. Needs the
#          repo, not a toolchain. Generated content derives from CODE, so it can
#          only go stale when code changes — and a code change always runs the
#          full gate.
#
# Why generated blocks exist at all: `entity-framework/SKILL.md` claimed "25
# entities" while the tree held 30. Hand-written counts in this repo rot within
# days, so no skill states a count in prose; counts live here.
#
#   overrides
#          Asserts the three deliberate skill overrides still hold: the house
#          skill exists at the root, names the skill it supersedes in its own
#          body, and does not collide with a root skill of the same name — plus
#          that the registry and CLAUDE.md's table describe the same set. Pure
#          shell, same as `lint`, and for the same reason: it must run on a
#          skills-only PR. See the section comment above `skill_overrides`.
#
#   generators
#          Asserts every generator wired into `run_regen` appears in the
#          generator table in the agent-skills standard, and nothing appears there
#          that is not wired. Pure shell, for the `lint` reason. See the section
#          comment above `generator_table_ids`.
#
# Usage: skills-check.sh [lint|regen|overrides|generators|check|all]
#   lint  = authoring rules + overrides + generators
#   check = regen + fail on drift (what CI runs)
#   all   = lint + check (what `make skills-check` runs)

set -euo pipefail

# Pin collation. Every generated block is built by piping `find` through `sort`,
# and some of those sorts compare PATHS — `entities/webhook/entity.yaml` against
# `entities/webhook_delivery/entity.yaml` differ at `/` (0x2F) versus `_` (0x5F).
# Under LC_ALL=C the `/` sorts first, which is what is committed and what CI
# (a C locale) regenerates. Under a UTF-8 locale punctuation drops to a lower
# collation level, so the comparison falls through to `d` versus `e` and
# `webhook_delivery` sorts first instead.
#
# Without this the gate's verdict depends on the developer's locale: `make check`
# reported a two-line `entities-index.md` diff on a macOS en_US.UTF-8 shell while
# CI was clean. That is a false-POSITIVE generator — the mirror of this repo's
# false-green rule, and worse in one way, because the obvious response is to
# commit the reordered file, which makes CI stale instead. One tree, one answer.
export LC_ALL=C

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

skills_dir=".claude/skills"
fail=0

red() { printf '\033[31m%s\033[0m\n' "$*"; }
note() { printf '  %s\n' "$*"; }
err() {
	red "FAIL $*"
	fail=1
}

# `err` for a caller whose STDOUT is a generated payload. `run_regen` runs every
# generator as `gen_x >"$tmp"`, so a generator that reports its own failure
# through `err` writes the words "FAIL regen: …" INTO the reference and splices
# them in as content — a corrupted block, committed, with the gate green on the
# next run because the file now matches the generator. Observed exactly once,
# which was enough. Diagnose on stderr; leave stdout empty so `splice` refuses.
err_quiet() {
	red "FAIL $*" >&2
	fail=1
}

# A finding that is true but must not fail the gate. The description cap is the
# only user: most skills already sit inside the warning band, so erroring would
# fail a clean tree, and the point is to be told BEFORE the edit that breaches
# it rather than by a red PR afterwards.
#
# Note this writes to STDOUT, unlike `err_quiet` above. That is safe only
# because no generator calls it: a warning spliced into a generated block would
# be the same corruption that function exists to prevent.
warn_count=0
warn() {
	printf '  warning: %s\n' "$*"
	((warn_count += 1)) || true
}

# A gate that dies silently reads exactly like a gate that passed. `pipefail`
# plus a `find` over a directory that happens not to exist is enough to do it,
# so name the line instead of exiting mute.
trap 'red "skills-check: aborted unexpectedly at line $LINENO"; exit 1' ERR

# `find` over a path that may not exist returns non-zero, and under `pipefail`
# that aborts the script. Entities legitimately have no `delete/` directory.
count_sql() {
	if [[ -d $1 ]]; then
		find "$1" -name '*.sql' | wc -l | tr -d ' '
	else
		echo 0
	fi
}

# ---------------------------------------------------------------------------
# lint
# ---------------------------------------------------------------------------

# Read one top-level scalar out of a SKILL.md frontmatter block. Values may be
# folded across continuation lines (a long `description:` usually is), so a
# continuation is any following line that is indented and not itself a key.
frontmatter_value() {
	local file=$1 key=$2
	awk -v key="$key" '
		NR == 1 && $0 != "---" { exit }
		NR == 1 { infm = 1; next }
		infm && $0 == "---" { exit }
		!infm { next }
		$0 ~ "^" key ":" {
			sub("^" key ": *", "")
			buf = $0
			found = 1
			next
		}
		found && /^[a-z_-]+:/ { found = 0 }
		found && /^[[:space:]]+[^[:space:]]/ {
			sub(/^[[:space:]]+/, " ")
			buf = buf $0
		}
		END { if (buf != "") print buf }
	' "$file"
}

# Every TOP-LEVEL key in a SKILL.md frontmatter block, one per line. Leading
# whitespace is what excludes a nested mapping's keys, so `stageshow`'s
# `metadata:` block contributes `metadata` and not `version`.
frontmatter_keys() {
	awk 'NR == 1 && $0 != "---" { exit }
	     NR == 1 { infm = 1; next }
	     infm && $0 == "---" { exit }
	     infm && /^[A-Za-z][A-Za-z0-9_-]*:/ { sub(/:.*/, ""); print }' "$1"
}

# A ` #` inside an unquoted YAML scalar opens a comment, so the value ENDS
# there — silently, with no parse error, in every YAML reader. `project-board`
# carried `user-owned project #2 "Product Roadmap"` and lost 872 of its 989
# characters: its whole trigger vocabulary and all four routing clauses, in both
# Claude Code and Codex. `frontmatter_value` reads the raw line, which is why
# the cap check and the description-budget table both reported the full length
# while the model was being shown a 117-character stub.
#
# Its own function so `skills-check-test.sh` can lift it and feed it both sides.
# The risk this rule carries is a predicate that quietly stops matching, and a
# rule that matches nothing is indistinguishable from a clean tree.
desc_truncated_by_yaml_comment() {
	[[ $1 == *' #'* ]]
}

# The sibling of the rule above, one character apart and the more expensive of
# the two: a ` #` truncates a value, this drops the whole SKILL.md.
#
# A `: ` inside an UNQUOTED plain scalar is not text, it is a parse error — YAML
# cannot distinguish it from a nested mapping key. The three agents that read
# these files disagree about it, which is why it survived so long: Claude Code
# and Codex both accept it, and Kimi Code CLI — a separate binary from
# `codex --profile kimi`, reading the same `.agents/skills/` bridge — rejects the
# file and loads nothing. Six skills were in that state when this rule was
# written (`aws-account-operations`, `entity-framework`, `land-pr`,
# `landing-page`, `skill-audit`, `sql-analysis`): authored once, bridged
# correctly, and invisible in one of the three agents anyway.
#
# Deliberately NOT a YAML parser — reimplementing one in bash would be the
# bigger risk. It covers the shapes that actually break a plain scalar:
#
#   - it contains `: `, or ends in `:`
#   - it opens a flow collection that is not the whole value, which is how
#     `land-pr` broke: `argument-hint: [a] [b]` is two juxtaposed sequences
#
# A quoted scalar is legal whatever it holds, so that returns false first. The
# bracket walk tracks depth rather than finding the first `]`, so a legitimately
# nested `[a, [b]]` is not failed; a bracket inside a quoted string inside a
# flow collection would still fool it, and no frontmatter here is that shape.
#
# Its own function so skills-check-test.sh can lift it and feed it both sides.
frontmatter_scalar_breaks_yaml() {
	local v=$1
	# No value means a block mapping follows (`metadata:`), not a scalar.
	[[ -n $v ]] || return 1
	# A quoted scalar may contain anything, including the above.
	case $v in
	'"'* | "'"*) return 1 ;;
	esac
	[[ $v == *': '* ]] && return 0
	[[ $v == *: ]] && return 0
	case $v in
	\[* | \{*)
		local i ch depth=0 closed=-1
		for ((i = 0; i < ${#v}; i++)); do
			ch=${v:$i:1}
			case $ch in
			'[' | '{') depth=$((depth + 1)) ;;
			']' | '}')
				depth=$((depth - 1))
				((depth > 0)) || { closed=$i; break; }
				;;
			esac
		done
		((closed < 0)) && return 0
		[[ ${v:$((closed + 1))} =~ [^[:space:]] ]] && return 0
		;;
	esac
	return 1
}

# Body = everything after the closing frontmatter delimiter.
body_line_count() {
	awk 'NR==1 && $0=="---" { infm=1; next }
	     infm && $0=="---" { infm=0; started=1; next }
	     started { n++ }
	     END { print n+0 }' "$1"
}

# Every relative markdown link target in a file, one per line.
relative_links() {
	grep -oE '\]\([^)#][^)]*\)' "$1" 2>/dev/null |
		sed -e 's/^](//' -e 's/)$//' |
		grep -vE '^(https?|mailto):' || true
}

lint_one_skill() {
	local dir=$1 name slug desc body_lines
	slug=$(basename "$dir")
	local skill_md="$dir/SKILL.md"

	# A vendored third-party skill (the agent-skills standard, "Vendored skills") is
	# kept byte-identical to its pinned upstream commit so a refresh is a plain
	# re-copy. The house authoring rules (evals.md, linked references, TOCs) are
	# not applied to it; its provenance record is checked instead.
	if [[ -f $dir/VENDORED.md ]]; then
		local field
		[[ -f $skill_md ]] || err "$slug: vendored skill has no SKILL.md"
		for field in Source Commit License Refresh Doing; do
			grep -qE "^\*\*$field:\*\*" "$dir/VENDORED.md" ||
				err "$slug: VENDORED.md has no '**$field:**' line"
		done
		return
	fi

	if [[ ! -f $skill_md ]]; then
		err "$slug: no SKILL.md"
		return
	fi

	# --- frontmatter ---------------------------------------------------------
	if [[ $(head -1 "$skill_md") != "---" ]]; then
		err "$slug: SKILL.md must open with '---' on line 1"
		return
	fi
	if [[ $(awk 'NR>1 && $0=="---" {print "y"; exit}' "$skill_md") != "y" ]]; then
		err "$slug: frontmatter is never closed with '---'"
		return
	fi

	name=$(frontmatter_value "$skill_md" name)
	desc=$(frontmatter_value "$skill_md" description)

	if [[ -z $name ]]; then
		err "$slug: frontmatter has no 'name'"
	else
		# The open spec requires name == directory name; Claude Code resolves
		# the slash command from one or the other, so a mismatch is ambiguous.
		[[ $name == "$slug" ]] || err "$slug: name '$name' != directory '$slug'"
		((${#name} <= 64)) || err "$slug: name is ${#name} chars (max 64)"
		[[ $name =~ ^[a-z0-9-]+$ ]] || err "$slug: name must be lowercase letters, digits and hyphens"
		case $name in
		*claude* | *anthropic*) err "$slug: name contains a reserved word" ;;
		esac
	fi

	if [[ -z $desc ]]; then
		err "$slug: frontmatter has no 'description'"
	else
		((${#desc} <= 1024)) || err "$slug: description is ${#desc} chars (max 1024)"
		# The cap is a cliff, and routing tails are what walk a description up to
		# it: every audit that closes an overlap appends "For X see y" to BOTH
		# sides, so unrelated work converges on the same line. Warn while there
		# is still room to trim deliberately, instead of discovering the limit as
		# a failed gate on an unrelated change.
		if ((${#desc} <= 1024)) && ((${#desc} > 964)); then
			warn "$slug: description is ${#desc} chars, $((1024 - ${#desc})) from the 1024 cap — trim before adding another routing tail"
		fi
		# First person in a description is injected into the system prompt and
		# measurably hurts discovery.
		if printf '%s' "$desc" | grep -qiE '(^| )(I |I'\''m|I can|we can|you can use this)'; then
			err "$slug: description must be third person (no 'I'/'you can use this')"
		fi
		# The cap above bounds how much of the description is too much. This one
		# bounds how much of it EXISTS: see desc_truncated_by_yaml_comment.
		if desc_truncated_by_yaml_comment "$desc"; then
			err "$slug: description contains ' #', which YAML reads as a comment — every character after it is discarded before the model sees it; reword (\"project number 2\") or quote the whole scalar"
		fi
	fi

	# Every top-level scalar, not only `description`: `land-pr` broke on
	# `argument-hint`, which no other rule here reads.
	local fm_key fm_val
	while IFS= read -r fm_key; do
		[[ -n $fm_key ]] || continue
		fm_val=$(frontmatter_value "$skill_md" "$fm_key")
		if frontmatter_scalar_breaks_yaml "$fm_val"; then
			err "$slug: frontmatter '$fm_key' is not valid YAML — an unquoted plain scalar may not contain ': ' or end in ':', and a flow sequence must be the whole value. Claude Code and Codex tolerate it; Kimi Code CLI drops the entire skill. Reword it (' — ' reads the same), or quote the scalar — but not 'description', which the generators extract with a bare sed that would carry the quotes into the generated tables."
		fi
	done < <(frontmatter_keys "$skill_md")

	# --- body size -----------------------------------------------------------
	body_lines=$(body_line_count "$skill_md")
	((body_lines < 500)) || err "$slug: SKILL.md body is $body_lines lines (max 500)"

	# --- evals ---------------------------------------------------------------
	if [[ ! -f $dir/evals.md ]]; then
		err "$slug: no evals.md (three scenarios required)"
	else
		local scenarios
		scenarios=$(grep -cE '^## ' "$dir/evals.md" || true)
		((scenarios >= 3)) || err "$slug: evals.md has $scenarios scenarios (minimum 3)"
	fi

	# --- links ---------------------------------------------------------------
	local f target path
	while IFS= read -r f; do
		[[ -n $f ]] || continue
		while IFS= read -r target; do
			[[ -n $target ]] || continue
			case $target in
			*\\*) err "$slug: ${f#"$dir/"} links with a backslash path ('$target')" ;;
			esac
			# Test the path without any `#fragment`, but report what was
			# written — a link to a heading in another file is legitimate.
			path=${target%%#*}
			[[ -n $path ]] || continue
			if [[ ! -e $(dirname "$f")/$path ]]; then
				err "$slug: ${f#"$dir/"} links to a missing file ('$target')"
			fi
		done < <(relative_links "$f")
	done < <(find "$dir" -name '*.md' -type f | sort)

	# --- reference depth -----------------------------------------------------
	# Claude partially reads files reached through another reference, so every
	# reference must hang directly off SKILL.md. A reference linking to a
	# sibling reference is the shape that breaks.
	#
	# All three spellings of that link have to be caught. This rule used to test
	# only the `references/`-prefixed forms, which missed the one an author is
	# actually most likely to write: from inside `references/`, the natural way
	# to point at a sibling is its bare filename.
	if [[ -d $dir/references ]]; then
		local base rel
		while IFS= read -r f; do
			base=$(basename "$f")
			while IFS= read -r target; do
				[[ -n $target ]] || continue
				rel=${target%%#*}            # drop any #fragment
				[[ -n $rel ]] || continue    # a bare #anchor is same-file
				rel=${rel#./}
				case $rel in
				references/*)
					# `references/x.md` or `./references/x.md`
					err "$slug: ${f#"$dir/"} links to a sibling reference ('$target') — references must be one level deep from SKILL.md"
					;;
				../*)
					# Upward — `../SKILL.md` is legitimate and worth encouraging.
					;;
				*/*)
					# Some other nested path; the missing-file check covers it.
					;;
				"$base")
					# A self-link, e.g. to one of this file's own headings.
					;;
				*)
					# No slash at all: a bare filename resolves inside this same
					# references/ directory, so it is a sibling link.
					err "$slug: ${f#"$dir/"} links to a sibling reference ('$target') — references must be one level deep from SKILL.md"
					;;
				esac
			done < <(relative_links "$f")
		done < <(find "$dir/references" -name '*.md' -type f | sort)

		# The other half of "one level deep": every reference must actually hang
		# off SKILL.md. A reference nothing links to is unreachable — Claude
		# never loads it, so it goes stale invisibly while still reading like
		# documentation. Only the no-sibling half of this rule was enforced
		# before, so the orphan half had never been checked.
		while IFS= read -r f; do
			if ! grep -qF "](references/$(basename "$f"))" "$dir/SKILL.md"; then
				err "$slug: references/$(basename "$f") is not linked from SKILL.md — an unreachable reference is never read"
			fi
		done < <(find "$dir/references" -name '*.md' -type f | sort)

		# --- table of contents on long references ---------------------------
		while IFS= read -r f; do
			local lines
			lines=$(wc -l <"$f" | tr -d ' ')
			if ((lines > 100)) && ! grep -qE '^## Contents' "$f"; then
				err "$slug: ${f#"$dir/"} is $lines lines and needs a '## Contents' table of contents"
			fi
		done < <(find "$dir/references" -name '*.md' -type f | sort)
	fi
}

run_lint() {
	echo "skills-check: lint"
	local found=0 dir
	for dir in "$skills_dir"/*/; do
		[[ -d $dir ]] || continue
		found=$((found + 1))
		lint_one_skill "${dir%/}"
	done
	# A gate that silently examines nothing is worse than no gate: the same
	# rename-detection reasoning as check-bff-authz.mjs.
	if ((found == 0)); then
		red "FAIL skills-check examined 0 skills — is $skills_dir still the right path?"
		exit 2
	fi
	if ((warn_count > 0)); then
		note "examined $found skills, $warn_count warning(s)"
	else
		note "examined $found skills"
	fi
}

# ---------------------------------------------------------------------------
# skill overrides
# ---------------------------------------------------------------------------
#
# Three general-purpose skills are deliberately overridden by house skills
# (CLAUDE.md, "general-purpose skills are overridden"). The override is NOT
# enforced by any loader setting — it rests on two things:
#
#   1. load order: root skills load at startup, nested and plugin skills later;
#   2. the house skill SAYING SO in its own body, so a session that has both
#      loaded is told which claim to discard.
#
# (1) is structural and cannot regress from an edit here. (2) is prose, and
# prose is exactly what rots: delete the sentence while refactoring a skill and
# the override silently degrades to a coincidence of load order, with nothing
# red anywhere. This is the gap mono#272 found the hard way — its catalog eval
# caught `frontend-design` shadowing the house skill, and that eval cost $8.55
# and 524s a run, which is why it ended up pinned and ignored rather than fixed.
#
# So: assert the cheap, deterministic half here instead. This gate does NOT
# prove a model picks the right skill at runtime — only an eval can do that, and
# the decision on record (issue #586) is not to buy one. It proves the override
# is still *stated* where it has to be stated for load order to mean anything.
#
# `overridden|house|where the overridden skill comes from`.
skill_overrides=(
	"rust-best-practice|rust-conventions|symposium-synced under rust/.claude/skills/"
	"frontend-design|frontend-design-system|plugin"
	"dataviz|frontend-data-ui|plugin"
)

# Body = everything after the closing frontmatter delimiter. Frontmatter is
# excluded deliberately: `description` is marketing copy for the skill router,
# while the override has to be stated where a session that already loaded the
# skill will read it.
skill_body() {
	awk 'NR==1 && $0=="---" { infm=1; next }
	     infm && $0=="---" { infm=0; started=1; next }
	     started' "$1"
}

# The mention must be BACKTICK-DELIMITED — `dataviz`, not dataviz. That is the
# house convention for a skill name, and insisting on it defeats two false
# passes that a bare substring search walks straight into. Both were observed on
# the real files while building this gate, not imagined:
#
#   1. A longer name containing the shorter one. `frontend-design-system`
#      contains `frontend-design`, so a bare search matches the house skill's
#      own name on its every self-reference.
#   2. A FILENAME containing it. `frontend-design-system`'s body cites
#      `frontend-design` as a bare word in a path or sentence — not the skill — and that alone
#      made the assertion pass while the override went unstated. This is the
#      more dangerous of the two, because stripping the skill's own name (the
#      obvious fix for 1) does nothing about it.
#
# Requiring the delimiters rules out both: a path like `frontend-design.md` has
# no backtick between `design` and `.md`, and `frontend-design-system` has none
# between `design` and `-system`.
names_skill_in_backticks() {
	local file=$1 skill=$2 body needle
	# Captured and THEN matched, never piped into `grep -q`.
	#
	# `set -euo pipefail` is on, and `grep -q` exits the instant it matches.
	# That closes the pipe while `awk` is still writing the rest of the body,
	# SIGPIPEs it, and makes the *pipeline* exit 141 — so a match that was
	# found reports as missing. It is a race, which is the worst version: it
	# passes locally and fails on one loaded CI runner, then passes on rerun.
	#
	# Not hypothetical. `agent skills (lint + drift)` failed on #606 claiming
	# `frontend-design-system` never names `frontend-design`, against a tree
	# where it does, at body line 26. Every house skill names its counterpart
	# early — body lines 26, 31 and 34 — leaving 6.6–9.7 kB still to write, so
	# all three are exposed and the largest body is the likeliest to fire.
	#
	# `tf-helpers-test.sh` already carries this warning for the same reason,
	# and ci-and-supply-chain's skill states the rule: `grep -c`, not `grep -q`,
	# on anything fed by a pipe. Bash pattern matching needs no pipe at all.
	body=$(skill_body "$file")
	needle="\`$skill\`"
	[[ $body == *"$needle"* ]]
}

# The rows of CLAUDE.md's override table, so the registry above and the document
# cannot drift apart in either direction.
override_table_rows() {
	awk '
		/^\| Skill \| Wrong here because \| Overridden by \|/ { intable = 1; next }
		intable && /^\|[ -]*-/ { next }
		intable && /^\|/ { print; next }
		intable { exit }
	' CLAUDE.md
}

run_override_check() {
	echo "skills-check: overrides"
	local entry overridden house origin rows row_count n

	for entry in "${skill_overrides[@]}"; do
		IFS='|' read -r overridden house origin <<<"$entry"

		# O1 — the house skill must exist, and at the ROOT. A house skill that
		# moved into a nested directory would lose the load-order race it is
		# relying on, which is not something its own prose would reveal.
		if [[ ! -f $skills_dir/$house/SKILL.md ]]; then
			err "override: house skill '$house' not found at $skills_dir/$house/SKILL.md (it must be a ROOT skill to load before '$overridden')"
			continue
		fi

		# O2 — the house skill must name the skill it supersedes, literally and
		# in backticks. "any generic data-visualization skill" leaves the model
		# to infer that `dataviz` is the thing meant; naming it removes the
		# inference step at the moment it matters.
		if ! names_skill_in_backticks "$skills_dir/$house/SKILL.md" "$overridden"; then
			err "override: '$house' never names \`$overridden\` in its body — the supersession is documented only in CLAUDE.md, so a session with both loaded is not told which claim to discard. State it in the body, with the skill name in backticks."
			note "  ($overridden comes from: $origin)"
		fi

		# O3 — a root skill of the same name is a genuine collision, not an
		# override: two skills would compete at the same load stage and the
		# winner is then undefined rather than decided.
		if [[ -e $skills_dir/$overridden ]]; then
			err "override: '$overridden' also exists as a ROOT skill at $skills_dir/$overridden — that is a name collision, not an override, and load order no longer decides it"
		fi
	done

	# O4 — the registry and CLAUDE.md's table must describe the same set.
	rows=$(override_table_rows)
	if [[ -z $rows ]]; then
		err "override: CLAUDE.md's override table is missing or its header changed — the registry in this script has nothing to agree with"
		return
	fi
	row_count=$(printf '%s\n' "$rows" | grep -c '^|')
	if ((row_count != ${#skill_overrides[@]})); then
		err "override: CLAUDE.md lists $row_count overridden skills, the registry in this script has ${#skill_overrides[@]} — add the new pair to BOTH"
	fi
	# Both names must appear on the SAME row. Searching the whole table for each
	# name independently would be satisfied by two half-correct rows — a table
	# that pairs `dataviz` with the wrong house skill still mentions both names
	# somewhere, and that is exactly the drift this is meant to catch.
	# Same no-`grep -q`-behind-a-pipe rule as `names_skill_in_backticks`. The
	# writer here is small enough that it has not been seen to fire, but the
	# shape is identical and a row list only grows.
	local matched
	for entry in "${skill_overrides[@]}"; do
		IFS='|' read -r overridden house origin <<<"$entry"
		matched=$(printf '%s\n' "$rows" | grep -F "\`$overridden\`" || true)
		if [[ $matched != *"\`$house\`"* ]]; then
			err "override: CLAUDE.md's table has no single row pairing \`$overridden\` with \`$house\`"
		fi
	done

	# The heading carries the count in words, and a hand-written count in this
	# repo goes stale within weeks — the reason every other count here lives in
	# a GENERATED block. This one cannot (it is a prose heading), so assert it.
	n=${#skill_overrides[@]}
	local word
	case $n in
	1) word=One ;; 2) word=Two ;; 3) word=Three ;; 4) word=Four ;;
	5) word=Five ;; 6) word=Six ;; 7) word=Seven ;; 8) word=Eight ;;
	*) word=$n ;;
	esac
	if ! grep -qF "### $word general-purpose skills are overridden" CLAUDE.md; then
		err "override: CLAUDE.md's heading does not read '### $word general-purpose skills are overridden' — the registry holds $n pairs, so the word in the heading is stale"
	fi

	note "checked $n documented skill overrides"
}

# ---------------------------------------------------------------------------
# the generator registry, asserted against the standard's table
#
# The table in the agent-skills standard cannot be generated — its "Derived from"
# column is prose explaining a design choice, which is the column worth reading.
# But the SET of generator ids in it is derivable from `run_regen`, and that set
# drifted: `post-merge-surface` was wired with the `land-pr` skill and never
# reached the table, so the standard documented 12 of 13 generators. Nothing
# failed, because a hand-maintained list disagreeing with code is exactly the
# defect class this gate exists for everywhere else.
#
# Same shape as the override registry check below it: derive both sides, diff
# the sets, name which side needs the edit. And the same no-`grep -q`-behind-a-
# pipe rule — `grep -c` or a captured match, never `-q`, on anything a pipe
# feeds.
# ---------------------------------------------------------------------------

# Generator ids as the standard's table lists them, one per row and in table
# order — not deduplicated, because a duplicate row is its own defect and
# `sort -u` is what hides it.
generator_table_ids_raw() {
	sed -n '/^| Generator | Target | Derived from |/,/^$/p' "$skills_dir/skill-writer/references/agent-skills.md" |
		sed -E -n 's/^\| `([a-z0-9-]+)` \|.*/\1/p'
}

# Generator ids as the standard's table lists them.
generator_table_ids() {
	generator_table_ids_raw | sort -u
}

# Generator ids actually spliced by `run_regen`. The splice call is the
# authority rather than the `gen_*` function names, because a function that is
# defined and never wired generates nothing.
wired_generator_ids() {
	grep -oE 'splice "[^"]+" [a-z0-9-]+ ' "${BASH_SOURCE[0]}" |
		awk '{ print $NF }' | sort -u
}

run_generator_check() {
	echo "skills-check: generators"
	local wired table missing extra n
	wired=$(wired_generator_ids)
	table=$(generator_table_ids)

	if [[ -z $wired ]]; then
		err "generators: found no splice calls in this script — the id extractor is broken, not the table"
		return
	fi
	if [[ -z $table ]]; then
		err "generators: $skills_dir/skill-writer/references/agent-skills.md has no generator table (or its header changed) — there is nothing for the wired set to agree with"
		return
	fi

	missing=$(comm -23 <(printf '%s\n' "$wired") <(printf '%s\n' "$table") || true)
	extra=$(comm -13 <(printf '%s\n' "$wired") <(printf '%s\n' "$table") || true)

	local id
	for id in $missing; do
		err "generators: \`$id\` is wired into run_regen but absent from the generator table in the agent-skills standard — add a row with its target and what it derives from"
	done
	for id in $extra; do
		err "generators: the generator table in the agent-skills standard lists \`$id\`, which no splice call wires — remove the row, or wire the generator"
	done

	# A duplicate row satisfies the set comparison above and still leaves the
	# table saying two different things about one generator. Two concurrent PRs
	# each adding the missing row is how it happens, and the set diff cannot see
	# it because both sides were deduplicated before being compared.
	local dup
	dup=$(generator_table_ids_raw | sort | uniq -d || true)
	for id in $dup; do
		err "generators: the generator table in the agent-skills standard lists \`$id\` more than once — keep one row"
	done

	n=$(printf '%s\n' "$wired" | grep -c . || true)
	note "checked $n wired generators against the standard's table"
}

# ---------------------------------------------------------------------------
# generators
# ---------------------------------------------------------------------------

gen_entities_index() {
	local total statements
	total=$(find entities -name entity.yaml | wc -l | tr -d ' ')
	statements=$(find entities -name '*.sql' | wc -l | tr -d ' ')
	echo "**${total} entities, ${statements} declared statements.**"
	echo
	echo "| Entity | Relation | Store | Key | Natural key | History |"
	echo "|---|---|---|---|---|---|"
	local f
	for f in $(find entities -name entity.yaml | sort); do
		awk '
			/^name:/        { sub(/^name: */, ""); name = $0 }
			/^relation:/    { sub(/^relation: */, ""); rel = $0 }
			/^key:/         { sub(/^key: */, ""); key = $0 }
			/^natural_key:/ { sub(/^natural_key: */, ""); nk = $0 }
			/^  stores:/    { sub(/^  stores: */, ""); st = $0 }
			/^  history:/   { sub(/^  history: */, ""); hist = $0 }
			END {
				gsub(/[\[\]"]/, "", key); gsub(/[\[\]"]/, "", nk); gsub(/[\[\]"]/, "", st)
				nkcell = (nk == "") ? "—" : "`" nk "`"
				hist = (hist == "true") ? "yes" : "—"
				printf "| `%s` | `%s` | %s | `%s` | %s | %s |\n", name, rel, st, key, nkcell, hist
			}
		' "$f"
	done
	echo
	echo "Declared statements per entity and operation:"
	echo
	echo "| Entity | create | read | update | delete |"
	echo "|---|---|---|---|---|"
	local d ent c r u x
	for d in $(find entities -name entity.yaml | sort); do
		ent=$(dirname "$d")
		c=$(count_sql "$ent/create")
		r=$(count_sql "$ent/read")
		u=$(count_sql "$ent/update")
		x=$(count_sql "$ent/delete")
		if ((c + r + u + x > 0)); then
			printf '| `%s` | %s | %s | %s | %s |\n' "$(basename "$ent")" "$c" "$r" "$u" "$x"
		fi
	done
}

gen_crate_map() {
	local count
	count=$(find rust/crates -maxdepth 1 -mindepth 1 -type d | wc -l | tr -d ' ')
	echo "**${count} crates.** The cargo workspace root is \`rust/\`, not the repo root."
	echo
	echo "| Crate | Purpose |"
	echo "|---|---|"
	local d name src line
	for d in $(find rust/crates -maxdepth 1 -mindepth 1 -type d | sort); do
		name=$(basename "$d")
		src="$d/src/lib.rs"
		[[ -f $src ]] || src="$d/src/main.rs"
		line=""
		if [[ -f $src ]]; then
			# The crate's own summary is its leading `//!` block. Accumulate
			# until the first sentence ends, so a purpose that wraps across
			# two lines doesn't get cut mid-clause.
			# Stop at a line-ending period, not at any period: section and
			# version tokens (`S4.7`, `0.1.2`) are full of interior dots.
			line=$(awk '
				/^\/\/!/ {
					s = $0
					sub(/^\/\/![[:space:]]?/, "", s)
					if (s == "" && buf == "") next
					if (s == "") { print buf; exit }
					buf = (buf == "") ? s : buf " " s
					n++
					if (s ~ /\.$/ || n >= 4) { print buf; exit }
					next
				}
				buf != "" { print buf; exit }
			' "$src")
			# Keep the first sentence only, and normalise table-breaking pipes.
			line=${line%%. *}
			line=${line%.}
			line=${line//|/\\|}
		fi
		[[ -n $line ]] || line=$(awk -F'"' '/^description *=/ { print $2; exit }' "$d/Cargo.toml" 2>/dev/null || true)
		[[ -n $line ]] || line="—"
		printf '| `%s` | %s |\n' "$name" "$line"
	done
	echo
	echo "Pinned versions that carry a reason (from \`rust/Cargo.toml\`):"
	echo
	echo '```toml'
	grep -E '^(rust-version|edition) *=' rust/Cargo.toml || true
	grep -E '^(duckdb|arrow|axum|tower|utoipa|tokio-postgres|tokio-postgres-rustls|rustls|sqlparser|thiserror|anyhow|reqwest) *=' rust/Cargo.toml || true
	echo '```'
	echo
	echo "## Workspace lints"
	echo
	echo "The policy, from \`[workspace.lints]\` and applied per crate via"
	echo "\`[lints] workspace = true\`:"
	echo
	echo '```toml'
	grep -E '^(unsafe_code|all|pedantic) *=' rust/Cargo.toml || true
	echo '```'
	echo
	echo "Every \`allow\` and the comment that justifies it. The reason is the"
	echo "comment in \`Cargo.toml\`, so an allow added without one shows up here as"
	echo "a blank cell:"
	echo
	echo "| Allowed | Why |"
	echo "|---|---|"
	# One row per comment block: an allow's reason is the comment above it, and
	# consecutive allows under one comment share it. Deriving the pairing rather
	# than restating the table in prose is the point — an allow added to
	# Cargo.toml lands here, and one added without a comment lands here visibly
	# unexplained.
	awk '
		/^\[workspace\.lints\.clippy\]/ { inblk = 1; next }
		inblk && /^\[/                  { inblk = 0 }
		!inblk                          { next }
		function flush() {
			if (keys != "") { printf "| %s | %s |\n", keys, why }
			keys = ""
		}
		/^#/ {
			s = $0
			sub(/^#[[:space:]]?/, "", s)
			if (keys != "") { flush(); why = "" }
			why = (why == "") ? s : why " " s
			next
		}
		/^[a-z_]+ *= *"allow"/ {
			keys = (keys == "") ? "`" $1 "`" : keys ", `" $1 "`"
			next
		}
		END { flush() }
	' rust/Cargo.toml
}

# The composition of `make check`, derived from the `check:` target rather than
# copied from it. The skill body said `fmt + lint + entities-check + test` for
# as long as that was true and kept saying it after `doc` and `skills-check`
# were added — the step list is exactly the kind of fact that drifts silently,
# because nothing fails when prose and Makefile disagree.
gen_rust_gates() {
	local prereqs step recipe pg
	prereqs=$(awk -F: '/^check:/ { print $2; exit }' Makefile)
	# shellcheck disable=SC2086 # deliberate word splitting into the step list
	set -- $prereqs
	printf '**`make check` = %s** — the `check:` target'"'"'s prerequisites, in order.\n' "$(
		printf '%s + ' "$@" | sed 's/ + $//'
	)"
	echo
	echo "| Step | Command |"
	echo "|---|---|"
	for step in "$@"; do
		recipe=$(awk -v t="$step" '
			$0 ~ "^"t":" { grab = 1; next }
			grab && /^\t/ { s = $0; sub(/^\t/, "", s); print s; next }
			grab          { exit }
		' Makefile | paste -sd';' - | sed 's/;/; /g')
		printf '| `%s` | `%s` |\n' "$step" "$recipe"
	done
	echo
	pg=$(grep -lF 'ER_TEST_PG_DSN' rust/crates/*/tests/*.rs 2>/dev/null | wc -l | tr -d ' ')
	printf '**%s test files read `ER_TEST_PG_DSN`** and self-skip without it.\n' "$pg"
}

# The invariant column for one E0xx code, out of the design doc's table.
# Long entries are clipped at a word boundary — these are signposts to the full
# rule, not a replacement for it.
rule_desc() {
	grep -E "^\| $1 \|" "$skills_dir/entity-framework/references/entity-framework-design.md" |
		awk -F'|' '{print $4}' |
		sed -e 's/^ *//' -e 's/ *$//' |
		awk '{
			if (length($0) <= 200) { print; exit }
			s = substr($0, 1, 200)
			sub(/[^ ]*$/, "", s)
			sub(/[ ,;—-]+$/, "", s)
			print s " …"
		}' || true
}

gen_diagnostics() {
	local implemented documented missing
	implemented=$(grep -oE '"E0[0-9]{2}"' rust/crates/er-sql-check/src/analyze.rs | tr -d '"' | sort -u)
	documented=$(grep -oE '^\| E0[0-9]{2}' "$skills_dir/entity-framework/references/entity-framework-design.md" | sed 's/^| //' | sort -u)
	echo "The statement-level diagnostics, emitted by \`er-sql-check\`. These are a"
	echo "separate family from the entity-level \`V\` rules in \`er-entity-lint\`."
	echo
	echo "**Implemented** (asserted against \`analyze.rs\`):"
	echo
	printf '%s\n' "$implemented" | while read -r c; do
		[[ -n $c ]] || continue
		printf '%s\n' "- \`$c\` — $(rule_desc "$c")"
	done
	missing=$(comm -13 <(printf '%s\n' "$implemented") <(printf '%s\n' "$documented") || true)
	echo
	if [[ -n ${missing//[[:space:]]/} ]]; then
		echo "**Documented but NOT implemented** — do not rely on these firing:"
		echo
		printf '%s\n' "$missing" | while read -r c; do
			[[ -n $c ]] || continue
			printf '%s\n' "- \`$c\` — $(rule_desc "$c")"
		done
		echo
		echo "This gap is the stated reason \`references/entity-lint-rules.md\` is generated"
		echo "and byte-compared: a documented-only rule is exactly the drift that"
		echo "produced it."
	else
		echo "Every documented code is implemented."
	fi
}

# The planner's version matrix. Every number in it had been hand-written, and
# one had already rotted: the skill said the rest of the workspace pulls arrow
# 58 while `rust/Cargo.toml` said 60. Derived from `rust/Cargo.lock`, which is
# tracked, so this still needs no toolchain.
#
# The non-obvious derivation is the sqlparser row. THREE majors resolve in this
# graph and only one is the parser: `ff_analysis::sqlparser` is
# `pub use datafusion_expr::sqlparser`, so the version that matters is the one
# datafusion-expr resolves — not the one ff-analysis also depends on directly,
# which is what `cargo tree` shows first and what makes this easy to get wrong.
gen_sql_planner_pins() {
	local lock=rust/Cargo.lock
	local manifest=rust/crates/er-sql-check/Cargo.toml
	local tag rev dfexpr arrow_ff arrow_duck arrow_ws sqlp_parser sqlp_ff sqlp_ws consumers

	tag=$(grep -oE 'tag = "[^"]+"' "$manifest" | head -1 | sed 's/.*"\(.*\)"/\1/')
	rev=$(awk '/^name = "ff-analysis"$/,/^$/' "$lock" |
		grep -oE 'tag=[^#]*#[0-9a-f]+' | sed 's/.*#//' | cut -c1-12)
	dfexpr=$(awk '/^name = "datafusion-expr"$/{getline; gsub(/"/,"",$3); print $3; exit}' "$lock")
	arrow_ff=$(awk '/^name = "ff-analysis"$/,/^$/' "$lock" | grep -oE 'arrow [0-9.]+' | awk '{print $2}')
	arrow_duck=$(awk '/^name = "duckdb"$/,/^$/' "$lock" | grep -oE 'arrow [0-9.]+' | awk '{print $2}')
	arrow_ws=$(grep -oE '^arrow = "[^"]+"' rust/Cargo.toml | sed 's/.*"\(.*\)"/\1/')
	sqlp_parser=$(awk '/^name = "datafusion-expr"$/,/^$/' "$lock" | grep -oE 'sqlparser [0-9.]+' | awk '{print $2}')
	sqlp_ff=$(awk '/^name = "ff-analysis"$/,/^$/' "$lock" | grep -oE 'sqlparser [0-9.]+' | awk '{print $2}')
	sqlp_ws=$(grep -oE '^sqlparser = \{ version = "[^"]+"' rust/Cargo.toml | sed 's/.*"\(.*\)"/\1/')
	# A set difference in spirit: which workspace crates declare the STANDALONE
	# sqlparser. `er-sql-check` appearing here would falsify the whole section.
	consumers=$(grep -lE '^sqlparser' rust/crates/*/Cargo.toml 2>/dev/null |
		sed 's|rust/crates/||; s|/Cargo.toml||' | sort | tr '\n' ' ')

	echo "| What | Version | Where it comes from |"
	echo "|---|---|---|"
	printf '| ff-analysis | `%s` (rev `%s`) | the `tag =` in `er-sql-check/Cargo.toml` |\n' "$tag" "$rev"
	printf '| DataFusion | %s | brought by ff-analysis; `datafusion-common`/`-expr` are pinned here to match |\n' "$dfexpr"
	printf '| arrow, analyser side | %s | brought by ff-analysis |\n' "$arrow_ff"
	printf '| arrow, rest of the workspace | %s (declared `%s`) | the major the `duckdb` crate pulls |\n' "$arrow_duck" "$arrow_ws"
	printf '| sqlparser, **the parser** | %s | datafusion-expr %s, re-exported as `ff_analysis::sqlparser` |\n' "$sqlp_parser" "$dfexpr"
	printf '| sqlparser, ff-analysis'"'"'s own | %s | ff-analysis depends on it directly; **not** what the re-export resolves to |\n' "$sqlp_ff"
	printf '| sqlparser, the workspace | %s | `rust/Cargo.toml`, for the crates below |\n' "$sqlp_ws"
	echo
	printf 'Crates declaring the standalone `sqlparser`: **%s**.\n' "${consumers% }"
	echo "\`er-sql-check\` is deliberately absent — it parses only through the"
	echo "re-export. If it ever appears in that list, this section is wrong."
}

# The description budget. `description` is capped at 1,024 chars and is a SHARED
# APPEND TARGET: closing an overlap means appending a clause to a neighbour's
# description, so unrelated audits all spend the same line and it reaches the cap
# with no single author having spent it. This block exists because the standard's
# remedy — "read the headroom before adding a tail" — was a command an author had
# to remember to run, and the failure mode when they did not was an audit
# DROPPING a routing clause it had already decided was needed.
#
# The threshold is DERIVED, not chosen: the median routing clause in the
# collection. A skill with less headroom than that cannot accept one more
# clause, which is the only question this table is asked.
gen_description_budget() {
	local f slug desc len median
	local -a rows=() clauses=()

	for f in "$skills_dir"/*/SKILL.md; do
		[[ -f $f ]] || continue
		slug=$(basename "$(dirname "$f")")
		desc=$(sed -n 's/^description: //p' "$f")
		len=${#desc}
		rows+=("$(printf '%4d\t%s\t%d' $((1024 - len)) "$slug" "$len")")
		# Every "for … see …" routing clause, one per line, to size a typical one.
		#
		# Split on BOTH sentence and clause boundaries, and ANCHOR the match on
		# "for". Without the anchor the match runs from the start of the segment
		# and swallows the entire what clause — the first version of this did
		# exactly that, reporting 868-character "routing clauses". The median
		# happened to survive because it ignores outliers, which is luck rather
		# than a design. awk does the splitting because BSD and GNU sed disagree
		# about `\n` in a replacement.
		while IFS= read -r c; do
			[[ -n $c ]] && clauses+=("${#c}")
		done < <(printf '%s' "$desc" | awk '{gsub(/\. |; /, "\n"); print}' |
			grep -oiE '^for [^;.]*see [a-z0-9-]+' || true)
	done

	median=$(printf '%s\n' "${clauses[@]}" | sort -n |
		awk '{a[NR]=$1} END{ if (NR==0) {print 0} else {print a[int((NR+1)/2)]} }')

	echo "Every skill's \`description\`, against the 1,024-character cap. Sorted by"
	echo "headroom, because the only question this table answers is whether a skill"
	echo "can still accept a routing clause."
	echo
	echo "| Headroom | Skill | Length |"
	echo "|---:|---|---:|"
	printf '%s\n' "${rows[@]}" | sort -n | while IFS=$'\t' read -r free slug len; do
		printf '| %s | `%s` | %s |\n' "$free" "$slug" "$len"
	done
	echo
	printf 'The median routing clause here is **%d characters**. Skills with less\n' "$median"
	echo "headroom than that cannot take another one — adding routing to any of them"
	echo "means trimming its **what** clause first, which is the documented fix:"
	echo
	local blocked=0
	while IFS=$'\t' read -r free slug len; do
		if ((free < median)); then
			printf -- '- `%s` — %d characters free\n' "$slug" "$free"
			blocked=$((blocked + 1))
		fi
	done < <(printf '%s\n' "${rows[@]}" | sort -n)
	((blocked > 0)) || echo "- none; every skill can take another clause"

	# --- the second cap, which is not ours -------------------------------
	#
	# Codex truncates each description at ~650 characters in its skill listing,
	# well under the 1,024 this repo enforces. Measured by diffing the authored
	# frontmatter against what `codex debug prompt-input` reports (Codex
	# 0.167.0): listed lengths clustered at 648–652, cut at a word boundary.
	# RE-MEASURE with that command rather than trusting this constant — it is
	# the one number here that another vendor can change.
	#
	# What matters is not the overflow but WHAT overflows. A description whose
	# only lost text is its `for … see …` routing tail measured fine: 9 of 10
	# routing trials selected correctly with 89% of all routing clauses
	# invisible. The one that misrouted lost TRIGGER vocabulary — a term it
	# shares with a neighbour that mentions the same term earlier. So the
	# column below reports which kind of text is being cut, which is the
	# actionable distinction.
	local cut=648
	local -a overflow=()
	for f in "$skills_dir"/*/SKILL.md; do
		[[ -f $f ]] || continue
		slug=$(basename "$(dirname "$f")")
		desc=$(sed -n 's/^description: //p' "$f")
		((${#desc} > cut)) || continue
		# Where the routing tail starts, if there is one. Everything from there
		# on is a `for … see …` clause; losing that is the benign case.
		local tail_at
		tail_at=$(printf '%s' "$desc" | awk -v cut="$cut" '{
			best = 0
			n = split($0, _unused, "")   # force a string context
			s = $0
			# Scan for the earliest "for " that is followed by " see " before the
			# next sentence end — the shape of a routing clause.
			for (i = 1; i <= length(s) - 4; i++) {
				if (tolower(substr(s, i, 4)) == "for " && (i == 1 || substr(s, i-1, 1) ~ /[ ;.]/)) {
					rest = substr(s, i)
					p = index(tolower(rest), " see ")
					if (p > 0 && p < 200) { best = i; break }
				}
			}
			print best
		}')
		if ((tail_at > 0 && tail_at <= cut)); then
			overflow+=("$(printf '%s\ttail only\t%d' "$slug" $((${#desc} - cut)))")
		else
			overflow+=("$(printf '%s\tTRIGGER TEXT\t%d' "$slug" $((${#desc} - cut)))")
		fi
	done

	echo
	printf 'A second cap applies that this repo does not set: **Codex truncates\n'
	printf 'each description at ~%d characters** in its skill listing. The question\n' "$cut"
	printf 'that matters is which KIND of text falls past it — a lost routing tail\n'
	echo "measured harmless, lost trigger vocabulary measured as a misroute."
	echo
	echo "| Skill | What is cut | Chars past the cut |"
	echo "|---|---|---:|"
	printf '%s\n' "${overflow[@]+"${overflow[@]}"}" | sort -t$'\t' -k2,2 -k3,3nr |
		while IFS=$'\t' read -r slug kind n; do
			[[ -n $slug ]] && printf '| `%s` | %s | %s |\n' "$slug" "$kind" "$n"
		done
	echo
	echo "\`TRIGGER TEXT\` is the row to act on, and only when the lost words are"
	echo "a term a neighbouring skill also uses — see the Codex bridge section for"
	echo "the measured case and why the other 26 rows were left alone."
}

gen_required_checks() {
	echo "Branch protection is repository configuration, not a file in the tree, so"
	echo "the required set cannot be derived here. What *is* derived: every job, the"
	echo "display name that becomes its status-check context, its timeout, and its"
	echo "\`if:\` condition verbatim — a job gated off pull requests can never be a"
	echo "required check. A \`detect\`-gated job reports \`skipped\`, which branch"
	echo "protection accepts as satisfied; that is the whole reason the pattern"
	echo "exists instead of a workflow-level \`paths:\` filter."
	echo
	echo "A context below containing \`\${{ matrix.* }}\` is a MATRIX job, and its"
	echo "per-leg names (\`plan (core)\`, …) **cannot be required**: when the job is"
	echo "skipped by its \`if:\`, GitHub reports one check-run under the literal,"
	echo "uninterpolated name shown here, so the per-leg contexts never report and"
	echo "a PR that skips the tier blocks forever. Require a non-matrix aggregator"
	echo "job instead — \`terraform plan\` is the worked example."
	echo
	local wf
	for wf in .github/workflows/*.yml; do
		# Skip COMPILER OUTPUT. `gh aw compile` writes `*.lock.yml` plus
		# `agentics-maintenance.yml`, all of which match this glob, carry a
		# dozen machine-generated jobs each, and are rewritten by every gh-aw
		# version bump — so enumerating them would churn this file and fail
		# both the `skills` job and `lint`'s drift step for no reader's
		# benefit. The agentic workflows are summarised from their `.md`
		# sources below instead, which is the file a human actually edits.
		#
		# Detected by the generated-file marker rather than by filename:
		# `*.lock.yml` alone missed `agentics-maintenance.yml`, and a
		# filename list would miss whatever the next gh-aw release adds.
		# Both markers are checked because the two files word it differently.
		if head -3 "$wf" | grep -qiE 'DO NOT EDIT|automatically generated'; then
			continue
		fi
		echo "### \`$(basename "$wf")\`"
		echo
		echo "| Job | Status-check context | Timeout | Condition |"
		echo "|---|---|---|---|"
		# Only keys inside the top-level `jobs:` mapping are jobs. Matching
		# two-space keys anywhere also catches `on:`'s triggers and
		# `defaults:`'s children.
		awk '
			/^[a-z][a-z0-9_-]*:/ { injobs = ($0 ~ /^jobs:/) ? 1 : 0 }
			!injobs { next }
			# A folded or literal `if:` carries its value on the FOLLOWING
			# lines, so it has to be collected rather than read off the key.
			# Without this the table reported the block indicator `>-` as the
			# condition — a generated reference stating something false, which
			# is the one thing a generated reference exists to prevent.
			incond {
				if ($0 ~ /^[[:space:]]*$/) next
				if ($0 ~ /^      /) {
					line = $0
					sub(/^[[:space:]]+/, "", line)
					cond = (cond == "") ? line : cond " " line
					next
				}
				incond = 0
			}
			/^  [a-z][a-z0-9_-]*:[[:space:]]*$/ {
				if (job != "") emit()
				job = $1; sub(/:$/, "", job)
				name = ""; tmo = "—"; cond = "always"
			}
			/^    name:/ { sub(/^    name: */, ""); name = $0 }
			/^    timeout-minutes:/ { sub(/^    timeout-minutes: */, ""); tmo = $0 }
			/^    if:/ {
				sub(/^    if: */, "")
				if ($0 ~ /^[|>][-+0-9]*[[:space:]]*$/) { cond = ""; incond = 1; next }
				cond = $0
			}
			END { if (job != "") emit() }
			function emit() {
				if (name == "") name = job
				# Escape and truncate at emit time: a collected block scalar is
				# not complete until the next key is reached.
				gsub(/\|/, "\\|", cond)
				if (length(cond) > 90) cond = substr(cond, 1, 90) " …"
				printf "| `%s` | %s | %s | `%s` |\n", job, name, tmo, cond
			}
		' "$wf"
		echo
	done

	# Agentic workflows, summarised from the `.md` sources rather than from the
	# `.lock.yml` the compiler emits. Two columns, because they are the two
	# questions a reviewer has: what fires it, and what it is permitted to do.
	# None of these may ever be a required check — they are non-deterministic by
	# construction, so a required one would make every merge wait on a model.
	# That is also why they are the only workflows here allowed a `paths:`
	# filter, which `rust.yml` and `frontend.yml` may not have.
	if compgen -G ".github/workflows/*.md" >/dev/null; then
		echo "### Agentic workflows (\`gh aw\`)"
		echo
		echo "Compiled to \`*.lock.yml\` by \`gh aw compile\`; edit the \`.md\`. **Never"
		echo "required checks** — a required check that waits on a model would gate every"
		echo "merge on a non-deterministic run, which is also why these are the only"
		echo "workflows in this repo permitted a \`paths:\` filter."
		echo
		echo "**Mode** is \`staged\` when the workflow sets \`safe-outputs.staged\`, meaning"
		echo "every output below is previewed in the run summary rather than applied: no"
		echo "comment posted, no branch pushed, no issue opened. That is how an agent is"
		echo "watched before it is trusted — remove the key to make it act."
		echo
		echo "| Workflow | Mode | Triggers | Safe outputs |"
		echo "|---|---|---|---|"
		local awf
		for awf in .github/workflows/*.md; do
			awk -v file="$(basename "$awf")" '
				# Frontmatter only. The body below the closing `---` is prose
				# and may legitimately contain lines that look like YAML keys.
				NR == 1 && /^---[[:space:]]*$/ { infm = 1; next }
				infm && /^---[[:space:]]*$/ { exit }
				!infm { next }
				# A column-0 key ends whichever section was being collected.
				/^[a-z][a-z0-9_-]*:/ {
					key = $0; sub(/:.*$/, "", key)
					section = (key == "on" || key == "safe-outputs") ? key : ""
					# `on: push` — an inline value, so there are no children.
					if (section == "on" && $0 ~ /^on:[[:space:]]*[^[:space:]]/) {
						val = $0; sub(/^on:[[:space:]]*/, "", val)
						add("on", val)
						section = ""
					}
					next
				}
				section != "" && /^  [a-z][a-z0-9_-]*:/ {
					k = $1; sub(/:$/, "", k)
					add(section, k)
				}
				END {
					printf "| `%s` | %s | %s | %s |\n", file,
						(staged ? "**staged**" : "live"),
						(trig == "" ? "—" : trig), (out == "" ? "—" : out)
				}
				function add(sec, v) {
					if (sec == "on") {
						trig = trig (trig == "" ? "" : ", ") "`" v "`"
						return
					}
					# `staged` previews every output instead of applying it, so
					# it belongs in the Mode column — it is the difference
					# between an agent that acts and one that rehearses.
					if (v == "staged") { staged = 1; return }
					# Config keys that can appear under `safe-outputs:` but are
					# not outputs. Not exhaustive by design: if a new one shows
					# up in the Safe outputs column of the generated table, that
					# is the signal to add it here.
					if (v ~ /^(github-token|github-app|target-repo|allowed-repos|allowed-domains|env|messages|jobs|scripts|steps|threat-detection|environment|max-bot-mentions|failure-issue-repo|report-failure-as-issue|report-failed-jobs)$/) return
					out = out (out == "" ? "" : ", ") "`" v "`"
				}
			' "$awf"
		done
		echo
	fi

	echo "### Advisory ignores in \`rust/deny.toml\`"
	echo
	echo "Each entry carries a prose justification in the file itself. These gates are"
	echo "**time-dependent** — they fail on advisories and yanks published since the"
	echo "last green run, so a failure is usually not caused by the diff."
	echo
	echo '```toml'
	awk '/^ignore *= *\[/ { p = 1 } p { print } /^\]/ { if (p) exit }' rust/deny.toml
	echo '```'
	echo
	echo "Allowed licenses (mirrored **verbatim** into"
	echo "\`frontend/scripts/check-licenses.mjs\` as \`DENY_TOML_ALLOW\`, then unioned"
	echo "with npm-specific additions — keep the two in step):"
	echo
	echo '```toml'
	awk '/^allow *= *\[/ { p = 1 } p { print } /^\]/ { if (p) exit }' rust/deny.toml
	echo '```'
}

gen_tiers_and_thresholds() {
	echo "### Coverage floor"
	echo
	echo "From \`frontend/vitest.config.ts\`, enforced per-PR by \`pnpm check\`"
	echo "(\`vitest run --coverage\`) and scoped to \`src/lib/**\`:"
	echo
	echo '```ts'
	grep -E 'thresholds:' frontend/vitest.config.ts | sed -e 's/^ *//' || true
	echo '```'
	echo
	echo "### The Playwright pin must match in two places"
	echo
	echo "The committed visual baselines were rendered inside the container image, so"
	echo "the image tag and the dependency version have to agree. Both are shown here"
	echo "so a divergence shows up as a diff in this file:"
	echo
	printf '%s\n' "- \`frontend/package.json\` → \`$(awk -F'"' '/"@playwright\/test"/ { print $4; exit }' frontend/package.json || true)\`"
	printf '%s\n' "- \`.github/workflows/frontend.yml\` → \`$( (grep -oE 'mcr\.microsoft\.com/playwright:v[0-9.]+-[a-z]+' .github/workflows/frontend.yml || true) | head -1)\`"
	echo
	echo "### Test tiers"
	echo
	echo "| Tier | Directory | Command |"
	echo "|---|---|---|"
	# Assigned with a plain case, never inside a command substitution: a
	# backtick-quoted command name nested in $(...) is parsed as a nested
	# substitution and gets EXECUTED. This generator is pure text.
	#
	# The tiers are listed in pipeline order rather than derived, because the
	# order carries meaning a glob would lose. The set difference below is what
	# makes the list safe: `driver` was missing from this table for as long as
	# the tier existed — a required status check with its own Postgres service,
	# absent from the one table that answers "where does this test go".
	local t cmd dir
	for t in unit component functional driver e2e; do
		[[ -d frontend/tests/$t ]] || continue
		case $t in
		unit | component) cmd='`pnpm test`' ;;
		functional) cmd='`pnpm test:e2e`' ;;
		driver) cmd='`pnpm test:driver` — needs a real PostgreSQL; outside `vitest.config.ts`'"'"'s `include`' ;;
		e2e) cmd='`make frontend-e2e` — release gate, needs Docker (~10 min)' ;;
		*) cmd='—' ;;
		esac
		printf '| %s | `frontend/tests/%s/` | %s |\n' "$t" "$t" "$cmd"
	done
	# Any test directory this generator does not know about. `mock-erserver`
	# and `support` are fixtures rather than tiers; anything else is a tier
	# nobody added here, and it says so in the table instead of being absent.
	for dir in frontend/tests/*/; do
		t=$(basename "${dir%/}")
		case $t in
		unit | component | functional | driver | e2e | mock-erserver | support) continue ;;
		esac
		printf '| %s | `frontend/tests/%s/` | **unmapped — add it to `gen_tiers_and_thresholds`** |\n' "$t" "$t"
	done
}

gen_design_tokens() {
	local css=frontend/src/app/globals.css
	echo "### Semantic tokens"
	echo
	echo "Values from \`:root\` and \`.dark\` in \`globals.css\`. The **utility** column"
	echo "is present only when the token has an \`@theme inline\` entry; a token with no"
	echo "utility cannot be used in a class name."
	echo
	echo "| Token | Light | Dark | Utility |"
	echo "|---|---|---|---|"
	# Token names in :root order. Only colour tokens are tabulated — a row is
	# emitted when the light value is an oklch() literal, which excludes
	# `--radius` and anything else non-chromatic that would show an empty Dark
	# column and read as a defect.
	local name light dark util
	while read -r name; do
		[[ -n $name ]] || continue
		light=$(awk -v n="--$name:" '/^:root/ { r = 1 } /^\.dark/ { r = 0 } r && index($0, n) { sub(/^[^:]*:[[:space:]]*/, ""); sub(/;.*$/, ""); print; exit }' "$css")
		[[ $light == oklch* ]] || continue
		dark=$(awk -v n="--$name:" '/^\.dark/ { d = 1 } /^@theme/ { d = 0 } d && index($0, n) { sub(/^[^:]*:[[:space:]]*/, ""); sub(/;.*$/, ""); print; exit }' "$css")
		if grep -qE "^[[:space:]]*--color-$name:" "$css"; then
			util="\`*-$name\`"
		else
			util="**none — unreachable from a class name**"
		fi
		printf '| `--%s` | `%s` | `%s` | %s |\n' "$name" "$light" "${dark:-**missing**}" "$util"
	done < <(awk '/^:root/ { r = 1; next } /^}/ { r = 0 } r && /^[[:space:]]*--/ { sub(/^[[:space:]]*--/, ""); sub(/:.*$/, ""); print }' "$css")
	echo
	echo "Any row whose Light or Dark value has a non-zero middle number carries"
	echo "chroma. \`--destructive\` is the single deliberate exception; a second one"
	echo "means the grayscale rule has been broken at the token level, where the"
	echo "ESLint rule cannot see it. A **missing** Dark value or an unreachable"
	echo "utility is likewise a defect rather than a choice."
	echo
	echo "Non-colour tokens are deliberately not tabulated above. The radius scale"
	echo "is the one that matters:"
	echo
	echo '```css'
	grep -E '^[[:space:]]*--radius(-[a-z]+)?:' "$css" | sed -e 's/^[[:space:]]*//' || true
	echo '```'
	echo
	echo "### Primitives installed"
	echo
	echo "\`src/components/ui/\`, with the shadcn generator settings from"
	echo "\`frontend/components.json\`:"
	echo
	echo '```json'
	# Only the settings that change what `shadcn add` writes.
	grep -E '"(style|rsc|tsx|baseColor|cssVariables|iconLibrary)"' frontend/components.json | sed -e 's/^[[:space:]]*//' || true
	echo '```'
	echo
	local f
	for f in frontend/src/components/ui/*.tsx; do
		[[ -e $f ]] || continue
		printf '%s\n' "- \`$(basename "$f" .tsx)\`"
	done
	echo
	echo "Anything absent from that list is **not** installed. Add it with"
	echo "\`pnpm dlx shadcn@latest add <name>\` rather than hand-rolling it, and check"
	echo "first whether a native element does the job with no new dependency."
	echo
	echo "### The palette ban"
	echo
	echo "The \`no-restricted-syntax\` selector in \`frontend/eslint.config.mjs\` that"
	echo "rejects Tailwind palette utilities, matched against both \`Literal\` and"
	echo "\`TemplateElement\` nodes:"
	echo
	echo '```'
	grep -oE 'selector: `Literal\[value=/[^`]*`' frontend/eslint.config.mjs | head -1 | sed -e 's/^selector: `//' -e 's/`$//' || true
	echo '```'
}

gen_screens_coverage() {
	local spec=frontend/tests/functional/a11y.spec.ts
	local routes gated missing
	# Route paths from the app router: strip the src/app prefix and /page.tsx
	# suffix, then drop (group) segments, which are organisational and do not
	# appear in a URL.
	routes=$(
		find frontend/src/app -name 'page.tsx' \
			| sed -e 's#^frontend/src/app##' -e 's#/page\.tsx$##' \
			| sed -e 's#/([^)]*)##g' \
			| sed -e 's#^$#/#' \
			| sort -u
	)
	# The gate addresses screens by `path:`. Tenant entries are template literals
	# interpolating the fixture org; public ones are plain quoted strings. BOTH
	# spellings must be read — matching only backticks reported the landing page
	# as unchecked while it was in fact swept, and a gate that cries wolf about a
	# covered screen is worse than one that stays quiet.
	gated=$(
		grep -oE 'path: (`[^`]*`|"[^"]*"|'"'"'[^'"'"']*'"'"')' "$spec" \
			| sed -e 's/^path: //' -e 's/^[`"'"'"']//' -e 's/[`"'"'"']$//' -e 's#\${ORG}#[org]#g' -e 's#\${DS}#[ds]#g' \
			| sort -u
	)
	echo "### Screens swept by the axe gate"
	echo
	if [[ -n ${gated//[[:space:]]/} ]]; then
		printf '%s\n' "$gated" | while read -r p; do
			[[ -n $p ]] || continue
			printf '%s\n' "- \`$p\`"
		done
	else
		echo "**None** — the \`SCREENS\` array parsed empty, which almost certainly means"
		echo "the spec changed shape rather than that the gate was emptied."
	fi
	missing=$(comm -23 <(printf '%s\n' "$routes") <(printf '%s\n' "$gated") || true)
	echo
	echo "### Routes NOT in the axe gate"
	echo
	if [[ -n ${missing//[[:space:]]/} ]]; then
		echo "Each of these renders without any accessibility check. See the prose above"
		echo "for the three categories that legitimately sit outside the sweep."
		echo
		printf '%s\n' "$missing" | while read -r p; do
			[[ -n $p ]] || continue
			printf '%s\n' "- \`$p\`"
		done
	else
		echo "Every route is swept."
	fi
}

# A `- `path`` line per entry, or $2 verbatim when the list is empty.
bullets() {
	if [[ -n ${1//[[:space:]]/} ]]; then
		printf '%s\n' "$1" | while read -r entry; do
			[[ -n $entry ]] || continue
			printf '%s\n' "- \`$entry\`"
		done
	else
		printf '%s\n' "$2"
	fi
}

# Files under `frontend/src` with a real call to the hook matched by $1, doc
# comment lines excluded.
#
# That exclusion is load-bearing rather than tidy. `column-table.tsx` names
# `useVirtualizer` in a comment explaining why that table is deliberately *not*
# virtualized, and the landing page names `useSearchParams()` in a comment
# about what `force-static` empties. A derived list that counted either would
# lie in the same direction as the prose it replaced, which is the one way a
# generated block is worse than nothing.
hook_call_sites() {
	grep -rnE "$1" frontend/src 2>/dev/null \
		| grep -vE '^[^:]+:[0-9]+:[[:space:]]*\*' \
		| cut -d: -f1 \
		| sort -u
}

# The tree facts `frontend-data-ui` used to assert in prose. Each was a
# cardinality claim — "the only virtualized surface", "no `loading.tsx`
# anywhere", "the only `useSearchParams`", "there is no `<table>`" — and the
# last of those was false by 23 components before an audit caught it. A zero or
# a one is a count like any other, so it is derived here instead of written
# down.
gen_dense_surface_facts() {
	local virt urls boundaries tables
	virt=$(hook_call_sites 'useVirtualizer\(')
	urls=$(hook_call_sites 'useSearchParams\(')
	boundaries=$(
		find frontend/src/app \
			\( -name 'loading.tsx' -o -name 'error.tsx' -o -name 'not-found.tsx' \) \
			2>/dev/null | sort
	)
	tables=$(grep -rl '<table' frontend/src 2>/dev/null | wc -l | tr -d '[:space:]')

	echo "### Virtualized surfaces"
	echo
	echo "Every \`useVirtualizer\` call site. A second entry here means the"
	echo "walkthrough below is no longer the only precedent, and whichever surface"
	echo "is closest to the new one is the better thing to copy:"
	echo
	bullets "$virt" "**None** — which contradicts the walkthrough below, so one of the two is wrong."
	echo
	echo "### Route-level loading, error and not-found boundaries"
	echo
	echo "Files named \`loading.tsx\`, \`error.tsx\` or \`not-found.tsx\` anywhere under"
	echo "\`frontend/src/app\`:"
	echo
	bullets "$boundaries" "**None.** Every one of those states is handled inline, per component."
	echo
	echo "### Client components that read the URL"
	echo
	echo "Every \`useSearchParams\` call site. While this list reaches no further"
	echo "than the login screen, no filtered or searched view in the tenant"
	echo "surface can be linked, bookmarked or restored by the back button:"
	echo
	bullets "$urls" "**None.** Nothing in the tree reads the URL."
	echo
	echo "### Semantic tables"
	echo
	echo "Files under \`frontend/src\` rendering a \`<table>\` element: **$tables**."
	echo "Tabular data gets a real \`<table>\`; the records browser's grid-of-rows is"
	echo "what virtualization costs, not the house pattern to copy by default."
}

gen_aws_footprint() {
	echo "### Resource types declared"
	echo
	echo "Distinct \`resource \"aws_*\"\` blocks across \`infra/terraform/\`, by count:"
	echo
	echo '```'
	# The `awk` reformat is load-bearing: **`uniq -c` pads its count to a
	# different width on BSD and GNU** (macOS `   9`, GNU `      9`). Leaving
	# that padding in the output made every line of this block differ by leading
	# whitespace between a developer's Mac and CI — the gate failed with exactly
	# 63 insertions and 63 deletions while the same commit regenerated
	# byte-identically on the machine that produced it. Nothing about the data
	# had changed.
	#
	# This is the same class as the LC_ALL collation bug this script already
	# guards against at the top: a generated artifact whose bytes depend on the
	# developer's platform is a gate that cannot be green for everyone at once.
	# Any generator that shells out to a coreutil should pin its own formatting
	# rather than inherit the tool's.
	#
	# The sort also uses PER-KEY modifiers (`-k1,1nr -k2,2`) rather than a global
	# `-rn`: a global `-n` makes key 2 numeric too, so all the names parse as 0
	# and equal-count rows are ordered only by sort's last-resort rule. Per-key
	# modifiers keep the count numeric-descending and make the name a plain
	# ascending string sort, which is a total order because the names are unique
	# after `uniq`.
	grep -rhoE 'resource "aws_[a-z0-9_]+"' infra/terraform \
		| sed -e 's/^resource "//' -e 's/"$//' \
		| sort | uniq -c \
		| awk '{ printf "%4d  %s\n", $1, $2 }' \
		| sort -k1,1nr -k2,2 || true
	echo '```'
	echo
	# A SET DIFFERENCE, deliberately, and it exists to retire a specific failure.
	# The skill carried a hand-written sentence — "there is no CloudFront, ACM,
	# Route 53, WAF, Athena, Glue, DynamoDB, Lambda, SQS, SNS, GuardDuty or
	# Security Hub in this account" — nine lines above the count table that listed
	# an Athena workgroup, a Glue catalog and three SNS resources.
	#
	# An asserted ABSENCE is the one claim the thing it describes cannot falsify.
	# A stale positive gets noticed when someone opens the resource; a stale
	# negative is only caught by a reader who already knew better. So the candidate
	# list is the input and the absences are DERIVED: a service adopted later drops
	# off this list on the next regeneration with nobody editing anything, which is
	# the `screens-coverage` shape the agent-skills standard recommends.
	#
	# Output order is the array's, not `sort`'s, so this block cannot shift under a
	# different LC_ALL the way the count table above once did.
	echo "### Services and resource types with nothing declared"
	echo
	echo "Derived by set difference against the types above. A service adopted later"
	echo "drops off this list on the next regeneration, so an absence here cannot rot."
	echo
	local -a candidates=(
		"ACM|aws_acm_"
		"API Gateway|aws_api_?gateway"
		"Athena|aws_athena_"
		"AWS Backup|aws_backup_"
		"CloudFront|aws_cloudfront_"
		"CloudWatch dashboards|aws_cloudwatch_dashboard"
		"CloudWatch metric alarms|aws_cloudwatch_metric_alarm"
		"CloudWatch metric filters|aws_cloudwatch_log_metric_filter"
		"Cost Explorer (aws_ce_*)|aws_ce_"
		"DynamoDB|aws_dynamodb_"
		"ElastiCache|aws_elasticache_"
		"Elastic Load Balancing|aws_alb|aws_lb"
		"EventBridge|aws_cloudwatch_event_|aws_scheduler_"
		"Glue|aws_glue_"
		"GuardDuty|aws_guardduty_"
		"Lambda|aws_lambda_"
		"Route 53|aws_route53"
		"SQS|aws_sqs_"
		"SNS|aws_sns_"
		"Security Hub|aws_securityhub_"
		"Step Functions|aws_sfn_"
		"WAF|aws_waf"
	)
	local declared absent=0 entry label pattern hits
	declared=$(grep -rhoE 'resource "aws_[a-z0-9_]+"' infra/terraform \
		| sed -e 's/^resource "//' -e 's/"$//' | sort -u || true)
	for entry in "${candidates[@]}"; do
		label=${entry%%|*}
		# Everything after the first field is an alternation of prefixes, so a
		# service spelled more than one way (aws_lb / aws_alb) counts as present
		# when EITHER appears.
		pattern=${entry#*|}
		# CAPTURE then match. Never pipe a writer into `grep -q`: `-q` exits on
		# the first hit, which SIGPIPEs the writer and returns 141 — so a FOUND
		# service reports as absent, the exact inversion this block exists to
		# prevent. `-c` consumes all of its input instead.
		# `scripts/ci/skills-check-test.sh` greps this file for the piped form
		# and fails on a reintroduction, including from inside a comment.
		hits=$(printf '%s\n' "$declared" | grep -cE "^($pattern)" || true)
		if [[ ${hits:-0} -eq 0 ]]; then
			printf -- '- %s\n' "$label"
			absent=$((absent + 1))
		fi
	done
	[[ $absent -gt 0 ]] || echo "- (every candidate service has at least one resource declared)"
	echo
	# The alerting surface gets its own derived block because the skill body's
	# strongest claim was about it, and that claim was wrong in the way a reader
	# could not catch: "zero metric alarms, zero metric filters, zero SNS topics"
	# survived a skim because two of the three were still true.
	echo "### The alerting surface"
	echo
	echo "| Resource type | Declared |"
	echo "|---|---|"
	local rt
	for rt in aws_cloudwatch_metric_alarm aws_sns_topic aws_sns_topic_subscription \
		aws_cloudwatch_dashboard aws_cloudwatch_log_metric_filter \
		aws_budgets_budget aws_cloudwatch_log_group; do
		printf '| `%s` | %s |\n' "$rt" \
			"$(grep -rhoE "resource \"$rt\"" infra/terraform | wc -l | tr -d ' ')"
	done
	echo
	echo "### Identity Center permission sets"
	echo
	echo "| Permission set | Session | Inline policy |"
	echo "|---|---|---|"
	# Split on the double quote so the value is a field rather than a regex
	# capture: a greedy `.*"` strip eats the value itself.
	awk -F'"' '
		/resource "aws_ssoadmin_permission_set"/ { inblock = 1; label = $4; name = ""; dur = "" }
		inblock && $1 ~ /^[[:space:]]*name[[:space:]]*=[[:space:]]*$/ { name = $2 }
		inblock && $1 ~ /session_duration[[:space:]]*=[[:space:]]*$/ { dur = $2 }
		inblock && name != "" && dur != "" {
			printf "%s\t%s\t%s\n", name, dur, label
			inblock = 0
		}
	' infra/terraform/modules/identity-center/main.tf | while IFS=$'\t' read -r name dur label; do
		# An inline policy is attached by referencing the permission set's
		# resource label, so the presence of that reference is the fact worth
		# showing: a set with no inline policy carries only its managed policy
		# attachments, and so has no deny statements at all.
		#
		# `p` MUST reset at every `resource "` line. Setting it once on the first
		# inline-policy block and leaving it set searches the whole rest of the
		# file, where every label appears again in its account assignment — which
		# reported all three sets as guarded, including the one that is not.
		if awk -v l="$label" '
			/^resource "/ { p = ($0 ~ /^resource "aws_ssoadmin_permission_set_inline_policy"/) }
			p && index($0, "aws_ssoadmin_permission_set." l ".arn") { found = 1 }
			END { exit !found }
		' infra/terraform/modules/identity-center/main.tf; then
			printf '| `%s` | %s | yes |\n' "$name" "$dur"
		else
			printf '| `%s` | %s | **none** |\n' "$name" "$dur"
		fi
	done
	echo
	echo "### The AWS SDK surface in the Rust workspace"
	echo
	echo "Every \`aws-*\` dependency, with the crate that declares it. All must stay"
	echo "\`default-features = false\`; the reasons are in \`bedrock-and-sdks.md\`."
	echo
	# [a-z0-9-] not [a-z-]: every SDK crate carries a version in its name
	# (aws-sdk-sesv2, aws-sdk-s3), and omitting the digits silently drops the
	# ones that matter most.
	local hits
	hits=$(grep -rhoE '^aws-[a-z0-9-]+' rust/crates/*/Cargo.toml | sort -u || true)
	if [[ -n ${hits//[[:space:]]/} ]]; then
		local crate
		echo "| Crate | Declared by |"
		echo "|---|---|"
		printf '%s\n' "$hits" | while read -r dep; do
			[[ -n $dep ]] || continue
			crate=$(grep -lE "^$dep = " rust/crates/*/Cargo.toml | sed -e 's#^rust/crates/##' -e 's#/Cargo.toml$##' | paste -sd, -)
			printf '| `%s` | `%s` |\n' "$dep" "$crate"
		done
	else
		echo "**No \`aws-*\` dependency in the workspace.**"
	fi
	echo
	echo "Any crate above whose declaration is missing \`default-features = false\`"
	echo "will fail the image build — \`aws-lc-sys\` needs a \`cmake\` the build image"
	echo "does not carry:"
	echo
	echo '```'
	grep -rhE '^aws-[a-z0-9-]+ = ' rust/crates/*/Cargo.toml \
		| grep -v 'default-features = false' || echo "(every aws-* declaration pins default-features = false)"
	echo '```'
	echo
	echo "### Terraform and provider pins"
	echo
	echo '```'
	# Any registry namespace, not just hashicorp/: the version alternative below
	# matches every `~>` pin, so narrowing the source alternative listed a bare
	# `version = "~> 5.0"` with no provider beside it the moment a third-party
	# provider was added (cloudflare, for the SES DNS records).
	grep -rhE 'required_version|source[[:space:]]*=[[:space:]]*"[a-z0-9-]+/|version[[:space:]]*=[[:space:]]*"~>' \
		infra/terraform/envs/*/versions.tf \
		| sed -e 's/^[[:space:]]*//' | sort -u || true
	echo '```'
	echo
	echo "### Where the engine's AWS access actually comes from"
	echo
	echo "Workload credentials, by mechanism — the thing to check first when a pod"
	echo "cannot reach AWS:"
	echo
	echo "| Mechanism | Count |"
	echo "|---|---|"
	printf '| `aws_eks_pod_identity_association` blocks | %s |\n' \
		"$(grep -rc 'resource "aws_eks_pod_identity_association"' infra/terraform --include='*.tf' | awk -F: '{ s += $2 } END { print s + 0 }')"
	printf '| `aws_iam_user` blocks (should be the lake key and the builder key only) | %s |\n' \
		"$(grep -rc 'resource "aws_iam_user"' infra/terraform --include='*.tf' | awk -F: '{ s += $2 } END { print s + 0 }')"
	printf '| `aws_iam_openid_connect_provider` blocks (GitHub Actions only; never for workloads) | %s |\n' \
		"$(grep -rc 'resource "aws_iam_openid_connect_provider"' infra/terraform --include='*.tf' | awk -F: '{ s += $2 } END { print s + 0 }')"
}

# Which workflows can never report on a pull request, and therefore what the
# post-merge loop is for. A set difference rather than a list: the interesting
# answer is the GAP between "runs after a merge" and "a PR could have caught
# it", and nobody has to remember to maintain a gap.
#
# Generated-ness is read from each file's own header with the same predicate
# scripts/ci/actionlint.sh uses, so the next gh-aw workflow is excluded without
# anyone touching this.
wf_on_block() {
	# The `on:` block's own 2-space keys. Deeper indents (`paths:`, `branches:`,
	# `types:`) are deliberately not matched, and a `#` comment cannot match
	# `[a-z_]`.
	awk '
		/^on:/            { inon = 1; next }
		inon && /^[^[:space:]]/ { inon = 0 }
		inon && /^  [a-z_]+:/ {
			sub(/^  /, "")
			sub(/:.*/, "")
			print
		}
	' "$1" | sort -u
}

# Every `dir:` under one job's `matrix.include`, as a stack path relative to
# infra/terraform. Scoped per job because `plan` and `apply` carry separate
# matrices and conflating them would report prod as planned when it is only
# applied through the second-hop role.
# Membership test with no subprocess at all. The obvious spelling — piping the
# list into a quiet exact-match grep — is forbidden in this file by
# scripts/ci/skills-check-test.sh, and rightly: a quiet grep closes the pipe at
# its first match, which SIGPIPEs the writer under `pipefail` and can report a
# FOUND match as missing. That test is textual, so this comment deliberately
# describes the bad form rather than spelling it. The needle is quoted inside
# the pattern so a path matches literally rather than as a glob.
in_list() {
	local needle=$1 list=$2
	[[ $'\n'$list$'\n' == *$'\n'"$needle"$'\n'* ]]
}

infra_job_stacks() {
	local wf=.github/workflows/infra.yml job=$1
	awk -v want="$job" '
		/^  [a-z][a-z0-9-]*:[[:space:]]*$/ { in_job = ($0 ~ "^  " want ":[[:space:]]*$") }
		in_job && /dir:[[:space:]]*infra\/terraform\// {
			sub(/.*dir:[[:space:]]*infra\/terraform\//, "")
			sub(/[[:space:]]*$/, "")
			print
		}
	' "$wf" | sort -u
}

gen_infra_coverage() {
	local stacks validate plan apply uncovered
	local s k sites

	# A stack is a directory with its own backend, which is what gives it a
	# separate state key. Deriving it from `*.tf` instead would count `modules/`.
	stacks=$(
		find infra/terraform -name backend.tf -not -path '*/.terraform/*' \
			| sed -e 's#^infra/terraform/##' -e 's#/backend\.tf$##' \
			| sort -u
	)
	# The validate tier is a shell loop, not a matrix: `for d in a b c; do`.
	#
	# SEARCHED IN BOTH PLACES THE LOOP CAN LIVE, because it has already moved
	# once: it was inline in infra.yml until the loop was extracted to
	# scripts/ci/tf-validate-all.sh, and this generator — which named only the
	# workflow — then rendered every stack as validate=— and reported
	# `bootstrap/organization` as covered by no tier. A silently WRONG
	# generated reference, which is worse than a stale one, because the gate
	# goes green on it the moment it is committed.
	validate=$(
		sed -n 's/.*for d in \(.*\); do.*/\1/p' \
			scripts/ci/tf-validate-all.sh .github/workflows/infra.yml 2> /dev/null \
			| tr ' ' '\n' | sed '/^$/d' | sort -u
	)
	# POSITIVE CONTROL. Zero stacks means the loop moved again, not that
	# nothing is validated — `infra.yml`'s validate tier is the only cover
	# `bootstrap/organization` has. Fail on stderr and emit nothing, so
	# `splice` refuses rather than committing an all-dashes column.
	if [[ -z ${validate//[[:space:]]/} ]]; then
		err_quiet "regen: infra coverage found no \`for d in …; do\` validate loop" \
			"in scripts/ci/tf-validate-all.sh or .github/workflows/infra.yml —" \
			"it has moved; point gen_infra_coverage at its new home"
		return 1
	fi
	plan=$(infra_job_stacks plan)
	apply=$(infra_job_stacks apply)

	echo "### Terraform stacks and the tiers that cover them"
	echo
	echo "| Stack | \`validate\` | \`plan\` | \`apply\` |"
	echo "|---|---|---|---|"
	printf '%s\n' "$stacks" | while read -r s; do
		[[ -n $s ]] || continue
		local v=— p=— a=—
		in_list "$s" "$validate" && v=yes
		in_list "$s" "$plan" && p=yes
		in_list "$s" "$apply" && a=yes
		printf '%s\n' "| \`$s\` | $v | $p | $a |"
	done
	echo

	# The gap: a stack no tier touches at all. `plan`-only and `validate`-only
	# are both deliberate and documented, so neither is reported as a gap —
	# only a stack that nothing checks.
	uncovered=$(
		comm -23 <(printf '%s\n' "$stacks") \
			<(printf '%s\n%s\n%s\n' "$validate" "$plan" "$apply" | sort -u) || true
	)
	if [[ -n ${uncovered//[[:space:]]/} ]]; then
		echo "**Checked by no tier:**"
		echo
		printf '%s\n' "$uncovered" | while read -r s; do
			[[ -n $s ]] || continue
			printf '%s\n' "- \`$s\` — in the tree with its own state key, and no job reads it."
		done
	else
		echo "Every stack in the tree is reached by at least one tier."
	fi
	echo

	echo "### Where each \`infra/k8s\` path is applied from"
	echo
	echo "| Path | Referenced by |"
	echo "|---|---|"
	for k in infra/k8s/*; do
		[[ -e $k ]] || continue
		# Every site that names the path, whatever the mechanism — kubectl for
		# plain manifests, helm for the chart directories, `kind create` for the
		# cluster config. Reporting the referencing file rather than guessing the
		# verb keeps this true as mechanisms change.
		# `|| true` is load-bearing under `pipefail`: a path referenced nowhere
		# makes grep exit 1, and that is the row this table exists to print.
		# Without it the whole regen dies on the first unreferenced path.
		sites=$(
			{ grep -rlF "$k" Makefile scripts .github/workflows 2>/dev/null || true; } \
				| { grep -vxF 'scripts/skills-check.sh' || true; } \
				| sort -u | tr '\n' ' ' | sed -e 's/ $//' -e 's/ /`, `/g'
		)
		if [[ -n $sites ]]; then
			printf '%s\n' "| \`${k#infra/k8s/}\` | \`$sites\` |"
		else
			printf '%s\n' "| \`${k#infra/k8s/}\` | **nothing in the tree** |"
		fi
	done
	echo

	echo "### NodePool manifests the cluster apply reaches"
	echo
	echo "\`kubectl apply -f <dir>\` is non-recursive, and \`cluster-objects\`"
	echo "names two directories: the top level, applied to whichever cluster is"
	echo "targeted, and \`<env>/\` for the pools that differ per environment."
	echo "Any other subdirectory is reached by nothing."
	echo
	echo "| Manifest | Applied |"
	echo "|---|---|"
	{ grep -rlF 'kind: NodePool' infra/k8s/nodepools 2>/dev/null || true; } | sort | while read -r s; do
		local dir rel
		dir=$(dirname "$s")
		rel=${s#infra/k8s/nodepools/}
		if [[ $dir == infra/k8s/nodepools ]]; then
			printf '%s\n' "| \`$rel\` | yes — every cluster |"
		# An env directory is applied, and which ones exist is derived from the
		# Makefile rather than listed here: `cluster-objects` applies
		# `nodepools/$(ER_ENV)/`, so a directory is an env directory exactly
		# when some env uses that name. Deriving it keeps this table honest if
		# a third environment appears, and keeps `measure/` out without an
		# exclusion list — nothing applies a directory no env is named after.
		elif [[ -d infra/terraform/envs/${dir#infra/k8s/nodepools/} ]]; then
			printf '%s\n' "| \`$rel\` | yes — \`${dir#infra/k8s/nodepools/}\` only |"
		else
			printf '%s\n' "| \`$rel\` | no — subdirectory no environment is named after |"
		fi
	done
}

gen_post_merge_surface() {
	local wf triggers name examined=0
	local -a after_merge=() pr_blind=()

	for wf in .github/workflows/*.yml; do
		[[ -f $wf ]] || continue
		# Same marker test as the pin checker: generated workflows are gh-aw's.
		# `grep -c`, never `grep -q`, on anything fed by a pipe — `-q` exits on
		# the first match and SIGPIPEs the writer, so a FOUND marker can report
		# as absent. Here that would silently include a gh-aw workflow in the
		# post-merge surface, which is the fail-OPEN direction.
		local generated
		generated=$(head -5 "$wf" | grep -ciE 'generated|DO NOT EDIT|gh aw' || true)
		((generated == 0)) || continue
		examined=$((examined + 1))
		name=$(basename "$wf" .yml)
		triggers=$(wf_on_block "$wf" | tr '\n' ' ')
		triggers=${triggers% }
		case " $triggers " in
		*" push "* | *" workflow_run "* | *" schedule "*)
			after_merge+=("$name|$triggers")
			case " $triggers " in
			*" pull_request "*) ;;
			*) pr_blind+=("$name|$triggers") ;;
			esac
			;;
		esac
	done

	echo "### Workflows that run after a merge to \`main\`"
	echo
	if ((examined == 0)); then
		# A generator that silently examines nothing is worse than none: the
		# same rename-detection reasoning the pin checker uses.
		echo "**None examined** — either \`.github/workflows/\` moved or every file"
		echo "matched the generated-header test. Fix the generator before trusting this."
		return
	fi
	echo "| Workflow | Triggers |"
	echo "|---|---|"
	local row
	for row in "${after_merge[@]}"; do
		printf '| `%s` | `%s` |\n' "${row%%|*}" "${row#*|}"
	done
	echo
	echo "### Of those, the ones no pull request can pre-test"
	echo
	if ((${#pr_blind[@]} == 0)); then
		echo "Every post-merge workflow also triggers on \`pull_request\`, so the main"
		echo "loop has no blind spot to cover. Verify before believing it."
	else
		echo "These carry no \`pull_request\` trigger, so a green PR says nothing about"
		echo "them. **This is the surface the main loop exists for.**"
		echo
		for row in "${pr_blind[@]}"; do
			printf '%s\n' "- \`${row%%|*}\` — \`${row#*|}\`"
		done
	fi
}

# The inventory table that the agent-skills standard and CLAUDE.md each carry.
# Derived from the frontmatter so adding a skill cannot leave either one stale —
# the defect that produced the "no skill states a count" rule in the first
# place. The two documents want different columns, so this takes a mode:
#
#   covers — `| Skill | Covers |`, from the description's first em-dash clause
#   doing  — `| Doing | Skill |`, from the description's "Use when" clause
#
# Both projections come from the same field, which is what keeps them agreeing.

# Clip a description clause to a readable summary: stop at a comma, but only one
# that sits OUTSIDE parentheses, and keep consuming clauses until the result is
# long enough to be informative.
#
# Both halves are load-bearing. A naive `${s%%,*}` cuts "the stage chain (ingest,
# standardize, match…)" to "the stage chain (ingest", and cuts "blocking rules,
# comparison levels, m/u probabilities…" to a useless "blocking rules". Commas
# and parentheses are ASCII, so awk is safe here under LC_ALL=C — unlike the
# em-dash, which is why the caller uses parameter expansion for that split.
clip_clause() {
	awk -v s="$1" -v min="${2:-60}" -v max="${3:-200}" '
		BEGIN {
			depth = 0
			cut = 0
			for (i = 1; i <= length(s); i++) {
				c = substr(s, i, 1)
				if (c == "(" || c == "[") depth++
				else if (c == ")" || c == "]") depth--
				else if ((c == "," || c == ";") && depth == 0 && i >= min) { cut = i - 1; break }
			}
			out = (cut > 0) ? substr(s, 1, cut) : s
			if (length(out) > max) {
				out = substr(out, 1, max)
				sub(/ [^ ]*$/, "", out)
				out = out "…"
			}
			# An odd backtick count means the clip landed inside an inline-code
			# span, which renders the rest of the table row as code. Back up to
			# before the opening backtick instead.
			n = gsub(/`/, "`", out)
			if (n % 2 == 1) {
				sub(/`[^`]*$/, "", out)
				sub(/[[:space:]]+$/, "", out)
				sub(/[[:punct:]]+$/, "", out)
				if (out != "") out = out "…"
			}
			print out
		}'
}

# The `er` CLI's whole command surface, from the clap definition that IS that
# surface. Hand-copying it is what produced the drift this generator replaces:
# the `profile` subcommand was missing from the skill entirely, `correct` was
# documented without its `--run-at`, `maintain` was credited with
# "catalog-option flags" it does not take, and — the one with teeth —
# `--expire-older-than` was presented as opt-in while clap defaults it to
# `7 days`, so the invocation the skill called safe expires snapshots.
#
# A default is the half most worth deriving: it is a decision recorded in one
# place and read from another, and nothing fails when the copy goes stale.
gen_er_command_surface() {
	local src=rust/crates/er-cli/src/cli.rs body

	if [[ ! -f $src ]]; then
		err_quiet "regen: $src is missing — the er command surface cannot be derived"
		return
	fi

	# The parse runs BEFORE a single header line is printed, and its emptiness
	# is checked here rather than left to `splice`. A generator whose preamble
	# is unconditional cannot fail loudly: the payload is never empty, so
	# `splice` accepts it and the block becomes a header with no table under it
	# — every subcommand erased, gate green. Observed, not hypothesised: it is
	# what renaming the `Command` enum produced while this guard sat in awk's
	# END block instead of out here.
	body=$(parse_er_command_surface "$src")
	if [[ -z ${body//[[:space:]]/} ]]; then
		err_quiet "regen: parsed no subcommands out of $src — the clap definition moved"
		return
	fi

	echo "Derived from \`$src\`: the clap \`Cli\` globals and"
	echo "every \`Command\` variant, each flag carrying the default the code"
	echo "declares. **required** means clap has no default and refuses the"
	echo "invocation without it; \`off\` is a boolean switch; \`optional\` is an"
	echo "\`Option<…>\` the stage treats as absent."
	echo
	echo "Each blurb is the subcommand's clap \`about\`, so this block answers"
	echo "*what*. Everything about *why* — which flags must travel together, which"
	echo "default is load-bearing — is prose outside these markers."
	echo
	printf '%s\n' "$body"
}

# Two inputs: er-versions first, so a `default_value = er_versions::MODE_FULL`
# resolves to the string an operator actually passes rather than to a path
# nobody can type.
parse_er_command_surface() {
	awk '
		function kebab(s,   out, i, c) {
			out = ""
			for (i = 1; i <= length(s); i++) {
				c = substr(s, i, 1)
				if (c ~ /[A-Z]/) { if (i > 1) out = out "-"; out = out tolower(c) }
				else out = out c
			}
			return out
		}
		function dashed(s,   t) { t = s; gsub(/_/, "-", t); return t }
		function resolve(c,   k) { k = c; sub(/^[A-Za-z_]+::/, "", k); return (k in consts) ? consts[k] : c }
		function flush_doc(   d) { d = doc; doc = ""; return d }

		# Pass 1: the string constants a default_value can point at.
		FNR == NR {
			if (match($0, /^pub const [A-Z_0-9]+: &str = "[^"]*"/)) {
				split($0, parts, /[ =]+/)
				key = parts[3]; sub(/:$/, "", key)
				val = $0; sub(/^[^"]*"/, "", val); sub(/".*$/, "", val)
				consts[key] = val
			}
			next
		}

		/^struct Cli \{/   { in_cli = 1; cmd = "globals"; order[++n] = cmd; about[cmd] = ""; next }
		/^enum Command \{/ { in_cmd = 1; next }
		/^\}/              { in_cli = 0; in_cmd = 0; next }
		!(in_cli || in_cmd) { next }

		/^[ \t]*\/\/\// {
			line = $0
			sub(/^[ \t]*\/\/\/[ ]?/, "", line)
			if (line == "") next
			doc = (doc == "" ? line : doc " " line)
			next
		}
		/^[ \t]*#\[arg\(/     { attr = $0; next }
		/^[ \t]*#\[command\(/ { doc = ""; next }
		/^[ \t]*\/\//         { next }

		# A variant with fields, then one without.
		in_cmd && /^    [A-Z][A-Za-z0-9]*[ ]*\{$/ {
			name = $1; sub(/[ ]*\{$/, "", name)
			cmd = kebab(name); order[++n] = cmd; about[cmd] = flush_doc(); attr = ""
			next
		}
		in_cmd && /^    [A-Z][A-Za-z0-9]*,$/ {
			name = $1; sub(/,$/, "", name)
			cmd = kebab(name); order[++n] = cmd; about[cmd] = flush_doc(); attr = ""
			next
		}

		/^[ \t]*[a-z_][a-z_0-9]*:[ ]/ {
			if (attr == "") { doc = ""; next }
			field = $1; sub(/:$/, "", field)
			type = $0; sub(/^[ \t]*[a-z_0-9]*:[ ]*/, "", type); sub(/,$/, "", type)
			flag = dashed(field)
			if (match(attr, /long = "[^"]*"/)) flag = substr(attr, RSTART + 8, RLENGTH - 9)
			if (match(attr, /default_value = "[^"]*"/))          dv = "`" substr(attr, RSTART + 17, RLENGTH - 18) "`"
			else if (match(attr, /default_value = [A-Za-z_:]+/)) dv = "`" resolve(substr(attr, RSTART + 16, RLENGTH - 16)) "`"
			else if (match(attr, /default_value_t = [^),]+/))    dv = "`" substr(attr, RSTART + 18, RLENGTH - 18) "`"
			else if (type ~ /^Option</)                          dv = "optional"
			else if (type == "bool")                             dv = "off"
			else if (type ~ /^Vec</)                             dv = "**required**, repeatable"
			else                                                 dv = "**required**"
			rows[cmd] = rows[cmd] sprintf("| `--%s` | %s |\n", flag, dv)
			flags++
			attr = ""; doc = ""
			next
		}

		END {
			# A parse that finds no commands, or commands with no flags, has
			# stopped tracking `cli.rs` rather than found a CLI with no flags.
			# Printing nothing is the signal; the caller turns it into a failure
			# before it prints a preamble that would hide it.
			if (n < 2 || flags == 0) exit 0
			for (i = 1; i <= n; i++) {
				cmd = order[i]
				if (cmd == "globals") print "### The globals, on every subcommand\n"
				else printf "### `er %s`\n\n%s\n\n", cmd, about[cmd]
				if (rows[cmd] != "") printf "| Flag | Default |\n|---|---|\n%s\n", rows[cmd]
				else print "No flags.\n"
			}
		}
	' rust/crates/er-versions/src/lib.rs "$src"
}

gen_skill_inventory() {
	local mode=$1
	local dir slug name desc cell manual
	case $mode in
	covers) printf '| Skill | Covers |\n|---|---|\n' ;;
	doing) printf '| Doing | Skill |\n|---|---|\n' ;;
	*)
		err "gen_skill_inventory: unknown mode '$mode'"
		return
		;;
	esac
	for dir in "$skills_dir"/*/; do
		[[ -f "$dir/SKILL.md" ]] || continue
		slug=$(basename "${dir%/}")
		name=$(frontmatter_value "$dir/SKILL.md" name)
		name=${name:-$slug}
		desc=$(frontmatter_value "$dir/SKILL.md" description)
		manual=""
		if grep -qE '^disable-model-invocation:[[:space:]]*true' "$dir/SKILL.md"; then
			manual=" (user-invoked only)"
		fi

		if [[ $mode == covers ]]; then
			# Everything after the first em-dash, clipped at the first comma.
			#
			# Done with parameter expansion rather than sed, deliberately: the
			# script runs under LC_ALL=C (see the top of this file), where an
			# em-dash is three bytes and `[^—]` is a negated set of those three
			# BYTES — so `s/^[^—]*— *//` silently fails to strip the prefix on
			# some descriptions and not others. `${desc#*—}` is byte-exact.
			cell=${desc#*—}
			[[ $cell != "$desc" ]] || cell=$desc # no em-dash: use the whole thing
			cell=${cell#"${cell%%[![:space:]]*}"}
			cell=${cell%%. *}
			cell=$(clip_clause "$cell")
		else
			# The first items of the trigger list — written to be the shortest
			# phrase that routes correctly, which is exactly this column's job.
			#
			# Three openings are in use and all three must be handled, because a
			# skill that matches none falls back to its whole description and
			# produces a table cell hundreds of characters wide: most say "Use
			# when", rust-conventions says "Use before", and pentest (the one
			# user-invoked skill) says "Invoke manually as".
			# Extract the trigger phrase FIRST, and only then trim to one
			# sentence. Doing both in one sed means a description with no
			# trigger phrase still gets shortened, so testing "did the string
			# change" cannot tell the two cases apart — which is how pentest
			# ended up falling through every branch.
			local trigger
			# A vendored skill's description is upstream's and is not edited, so
			# its routing phrase comes from VENDORED.md's '**Doing:**' line.
			if [[ -f $dir/VENDORED.md ]] && grep -qE '^\*\*Doing:\*\* ' "$dir/VENDORED.md"; then
				cell=$(sed -nE 's/^\*\*Doing:\*\* +//p' "$dir/VENDORED.md" | head -1)
				printf '| %s%s | `%s` |\n' "$cell" "$manual" "$name"
				continue
			fi
			trigger=$(printf '%s\n' "$desc" | sed -E 's/^.*[Uu]se (when|before) //')
			if [[ $trigger == "$desc" ]]; then
				trigger=$(printf '%s\n' "$desc" | sed -E 's/^.*[Ii]nvoke manually as /invoking /')
			fi
			if [[ $trigger == "$desc" ]]; then
				# No trigger phrasing at all: fall back to the em-dash clause
				# rather than to the whole description.
				trigger=${desc#*—}
				trigger=${trigger#"${trigger%%[![:space:]]*}"}
			fi
			cell=$(clip_clause "${trigger%%. *}" 40 110)
		fi
		if [[ $mode == covers ]]; then
			printf '| `%s` | %s%s |\n' "$name" "$cell" "$manual"
		else
			printf '| %s%s | `%s` |\n' "$cell" "$manual" "$name"
		fi
	done
}

# ---------------------------------------------------------------------------
# regen
# ---------------------------------------------------------------------------

# Replace the content between `<!-- BEGIN GENERATED: id -->` and
# `<!-- END GENERATED: id -->` in $1 with stdin, leaving surrounding prose alone.
splice() {
	local target=$1 id=$2 payload=$3
	local start="<!-- BEGIN GENERATED: $id -->"
	local end="<!-- END GENERATED: $id -->"

	if [[ ! -f $target ]]; then
		err "regen: $target does not exist"
		return
	fi
	if ! grep -qF "$start" "$target" || ! grep -qF "$end" "$target"; then
		err "regen: $target is missing the '$id' GENERATED markers"
		return
	fi
	# An empty payload means the generator broke. Splicing it in would quietly
	# erase the reference and still report success.
	if [[ ! -s $payload ]]; then
		err "regen: the '$id' generator produced no output"
		return
	fi

	# Adding a generator takes TWO edits — the splice call here and an entry in
	# `generated_files`, which is what the drift check iterates. Forgetting the
	# second produces a file that regenerates locally and is never checked for
	# staleness, so a stale committed block passes CI. That is not
	# hypothetical: `infra-coverage` shipped in exactly that state and the hole
	# only surfaced from mutating a source and watching the gate stay green.
	# The splice knows the target, so it can assert the pairing itself.
	local known=0 g
	for g in "${generated_files[@]}"; do
		[[ $g == "$target" ]] && known=1 && break
	done
	if ((known == 0)); then
		err "regen: '$target' is spliced but missing from generated_files, so the drift check would never see it"
		return
	fi

	local tmp
	tmp=$(mktemp)
	awk -v start="$start" -v end="$end" -v payload="$payload" '
		index($0, start) { print; while ((getline line < payload) > 0) print line; close(payload); skip = 1; next }
		index($0, end)   { skip = 0; print; next }
		!skip            { print }
	' "$target" >"$tmp"
	mv "$tmp" "$target"
}

# The observability surface: the roles, the gates, and the derived gap between
# what emits structured events and what still only reaches bare stderr.
#
# The metrics themselves are deliberately NOT here. `references/telemetry-reference.md`
# is generated from `er-obs/src/registry.rs` by that crate's own test and
# byte-compared by it, so a second generator over the same source would be a
# second thing to keep in step for no gain — the skill links to that file
# instead.
#
# The third block is the one no other gate owns. `collector_allowlist.rs` checks
# declared attributes against the collector's allowlist and `cardinality.rs`
# checks the registry's dimensions, but nothing anywhere derives how far
# structured logging has actually reached. The module docs stated that count in
# prose once and it was wrong within weeks, which is the exact defect a set
# difference fixes.
gen_obs_surface() {
	local crates=rust/crates
	local roles gates structured bare both onlybare emitters collectors

	# Which side of the split each crate sits on, read off the manifests. Both
	# crates' module docs say `er-obs` is for "binaries", and that is not the
	# line: `er-cli` and `er-server` each have a `lib.rs` and each take it. The
	# real rule is entry point versus engine, and a manifest cannot describe it
	# wrongly the way a doc comment can.
	emitters=$(
		grep -l 'er-span' "$crates"/*/Cargo.toml 2>/dev/null |
			xargs -n1 dirname | xargs -n1 basename |
			grep -vxE 'er-span|er-obs' | sort -u || true
	)
	collectors=$(
		grep -lE '^er-obs' "$crates"/*/Cargo.toml 2>/dev/null |
			xargs -n1 dirname | xargs -n1 basename | sort -u || true
	)

	# `Role::service_name`'s match arms: `Self::Api => "er-api",`. Read from the
	# impl rather than from a hand-kept list, because a new entry point is
	# precisely when this table would otherwise go stale.
	roles=$(
		sed -n '/const fn service_name/,/^    }/p' "$crates/er-obs/src/init.rs" 2>/dev/null |
			grep -oE 'Self::[A-Za-z]+ => "[^"]+"' |
			sed -E 's/Self::([A-Za-z]+) => "(.*)"/\1|\2/' || true
	)

	# Every test whose own module doc names it as enforcing a telemetry rule.
	# Matched on the file's leading `//!` so a renamed test is still found and a
	# test that stops being about telemetry drops out on its own.
	#
	# The summary is the first *sentence*, assembled from the whole leading
	# paragraph rather than from `head -1`. House style wraps a module doc at the
	# column, so `head -1` clipped `span_boundaries.rs` to "no bare
	# `spawn_blocking` in the" — a description that stops before its own object
	# reads as a broken generator, which is how it was caught.
	gates=$(
		grep -lE '^//!' "$crates"/er-obs/tests/*.rs "$crates"/er-schema/tests/*.rs 2>/dev/null |
			while read -r f; do
				local summary
				summary=$(
					awk '/^\/\/![[:space:]]*$/ { exit }
					     /^\/\/!/ { sub(/^\/\/! ?/, ""); printf "%s ", $0; next }
					     { exit }' "$f" |
						sed -E -e 's/\. .*$/./' -e 's/[[:space:]]+$//'
				)
				printf '%s\n' "$summary" |
					grep -qiE 'span|metric|telemetr|pii|allowlist|cardinalit|blocking hop' &&
					printf '%s|%s\n' "${f#rust/crates/}" "$summary"
			done || true
	)

	# Product sources only. `tests/`, `benches/` and `examples/` are excluded for
	# the reason `span_boundaries.rs` excludes the first two: an orphaned span or
	# a bare println in a harness costs nothing, and an example's stderr output
	# is user-facing by definition, so listing it as a gap would be noise that
	# nobody can act on.
	structured=$(
		grep -rlE '\btracing::(trace|debug|info|warn|error)!|\b(trace|debug|info|warn|error)_span!' \
			--include='*.rs' "$crates" 2>/dev/null |
			grep -vE '/(tests|benches|examples)/' | sed "s#^$crates/##" | sort -u || true
	)
	bare=$(
		grep -rlF 'eprintln!' --include='*.rs' "$crates" 2>/dev/null |
			grep -vE '/(tests|benches|examples)/' | sed "s#^$crates/##" | sort -u || true
	)
	# Wrapped: a `comm` over an empty side is the gap case, and the gap case is
	# the one that must not kill the regen under `set -o pipefail`.
	both=$(comm -12 <(printf '%s\n' "$bare") <(printf '%s\n' "$structured") || true)
	onlybare=$(comm -23 <(printf '%s\n' "$bare") <(printf '%s\n' "$structured") || true)

	# Split the bare-stderr set by whether anyone is WATCHING the terminal,
	# which is the only question `eprintln!` versus `tracing::warn!` turns on.
	#
	# The first version of this block reported `onlybare` flat, and that was a
	# measurement pointing the wrong way: six of its seven entries were
	# `er-entity`'s lint report, `dedupe`'s "wrote N rows", and a usage message
	# — all correct, one of them asserted by tests — so "close the gap" would
	# have meant breaking them. A metric that makes correct code look like debt
	# is worse than no metric.
	#
	# Classification comes from `Role::is_unattended`, a value in `er-obs` with
	# its own test, NOT from the role's doc comment. Every claim this skill
	# borrowed from a module doc has so far been wrong, so the arms are read
	# from the match and a crate with no `er_obs::init` call at all is dropped:
	# it has no subscriber, so it physically cannot emit an event and listing it
	# is noise.
	local unattended_roles attended_roles unattended_gap attended_gap nosub
	unattended_roles=$(
		sed -n '/const fn is_unattended/,/^    }/p' "$crates/er-obs/src/init.rs" 2>/dev/null |
			grep -E '=> true' | grep -oE 'Self::[A-Za-z]+' | sed 's/Self:://' | sort -u || true
	)
	attended_roles=$(
		sed -n '/const fn is_unattended/,/^    }/p' "$crates/er-obs/src/init.rs" 2>/dev/null |
			grep -E '=> false' | grep -oE 'Self::[A-Za-z]+' | sed 's/Self:://' | sort -u || true
	)
	# file -> the Role(s) whose process it runs in.
	#
	# Resolved per BINARY, not per crate, and that distinction is the whole
	# correctness of this split. `er-server/src/bin/er-openapi-check.rs` is its
	# own binary that installs no subscriber, but a crate-wide grep finds every
	# role `er-server` installs anywhere and so filed a build tool under
	# `Role::Api/Dispatcher/Task` — i.e. told the reader to convert the one file
	# in the set where a `tracing` event would go nowhere at all.
	#
	# So: a `src/bin/<name>.rs` or a `src/main.rs` IS an entry point, and only
	# its own text counts. Anything else is library code shared by whatever
	# binaries the crate ships, where the crate-wide answer is the right one.
	file_roles() {
		local f=$1 crate=${1%%/*} path="$crates/$1"
		case $f in
		*/src/bin/*.rs | */src/main.rs)
			grep -oE 'er_obs::Role::[A-Za-z]+' "$path" 2>/dev/null |
				sed 's/er_obs::Role:://' | sort -u || true
			;;
		*)
			grep -rhoE 'er_obs::Role::[A-Za-z]+' "$crates/$crate/src" 2>/dev/null |
				sed 's/er_obs::Role:://' | sort -u || true
			;;
		esac
	}
	unattended_gap=""
	attended_gap=""
	nosub=""
	if [[ -n ${onlybare//[[:space:]]/} ]]; then
		while read -r f; do
			[[ -n $f ]] || continue
			local rs verdict label
			rs=$(file_roles "$f")
			if [[ -z ${rs//[[:space:]]/} ]]; then
				nosub+="- \`$f\` — installs no subscriber"$'\n'
				continue
			fi
			# Pre-rendered with its own markup, because `bullets` wraps its
			# payload in backticks and a second pair nests into literal ones.
			label=$(printf '%s' "$rs" | sort -u | tr '\n' '/' | sed 's#/$##')
			# Membership by pure-bash substring match on a newline-delimited
			# set, rather than by piping the set into a quiet grep. That form
			# exits on its first match, SIGPIPEs the writer, and makes the
			# pipeline return 141 — so a role that IS deployed can report as
			# interactive. `scripts/ci/skills-check-test.sh` detects the pattern
			# by name and caught it here, which is also why this comment
			# describes it in words: the detector is textual, so spelling the
			# construct out in a comment trips it just as the real thing does.
			verdict=attended
			while read -r r; do
				[[ -n $r ]] || continue
				[[ $'\n'$unattended_roles$'\n' == *$'\n'"$r"$'\n'* ]] && verdict=unattended
			done <<<"$rs"
			if [[ $verdict == unattended ]]; then
				unattended_gap+="- \`$f\` — \`Role::$label\`"$'\n'
			else
				attended_gap+="- \`$f\` — \`Role::$label\`"$'\n'
			fi
		done <<<"$onlybare"
	fi
	unattended_gap=${unattended_gap%$'\n'}
	attended_gap=${attended_gap%$'\n'}
	nosub=${nosub%$'\n'}

	echo "### Which side of the split each crate is on"
	echo
	echo "**Depend on \`er-span\` — they emit and collect nothing.** Instrumenting"
	echo "one of these must not reach for \`er-obs\`:"
	echo
	bullets "$emitters" "**None**, which would mean nothing in the engine is instrumented."
	echo
	echo "**Depend on \`er-obs\` — they own an entry point and install the"
	echo "subscriber.** Note this is not the same as \"binary crates\": some of"
	echo "these have a \`lib.rs\` too, which is why both crates' module docs"
	echo "describing the line as bin-versus-lib are imprecise:"
	echo
	bullets "$collectors" "**None**, which would mean no process installs a subscriber."
	echo
	echo "### Roles"
	echo
	echo "What each entry point passes to \`er_obs::init\`, the \`service.name\` and"
	echo "\`er.role\` it therefore reports as, and whether the Helm charts ship it"
	echo "as a workload. The last column is why \"unattended\" is not \"deployed\":"
	echo "a local tool can still be unattended."
	echo
	echo "| \`Role\` | \`service.name\` | Ships as a workload |"
	echo "|---|---|---|"
	if [[ -n ${roles//[[:space:]]/} ]]; then
		printf '%s\n' "$roles" | while IFS='|' read -r variant service; do
			[[ -n $variant ]] || continue
			# Does anything in the Helm charts name this service? A role that
			# ships has a workload; one that does not is a local tool. Derived
			# rather than stated, because "no deployment" is an ABSENCE, and an
			# absence nobody records reads as an oversight — which is how
			# `Mcp` nearly acquired a Deployment it must never have (stdio
			# transport, and a single-org credential).
			local ships
			if grep -rqlF "$service" infra/k8s/charts 2>/dev/null; then
				ships="yes"
			else
				ships="**no** — local tool"
			fi
			printf '| `%s` | `%s` | %s |\n' "$variant" "$service" "$ships"
		done
	else
		echo "| — | the \`service_name\` match could not be read; the impl has moved | — |"
	fi
	echo
	echo "### The gates"
	echo
	echo "Every test enforcing a rule in the skill body, with the first line of"
	echo "its own module doc. Each is a **module of its crate's single test"
	echo "binary**, not a target of its own, so run one with"
	echo "\`cargo test -p <crate> --test main <module>\`:"
	echo
	if [[ -n ${gates//[[:space:]]/} ]]; then
		printf '%s\n' "$gates" | while IFS='|' read -r path summary; do
			[[ -n $path ]] || continue
			printf -- '- `%s` — %s\n' "$path" "$summary"
		done
	else
		echo "- **None found**, which cannot be right — the test trees have moved"
		echo "  and this block is no longer reading them."
	fi
	echo
	echo "### Structured events versus bare stderr"
	echo
	echo "A \`tracing\` event carries its span ancestry, so it is attributable to a"
	echo "run, a job and a tenant; an \`eprintln!\` is a line on stderr and nothing"
	echo "more. Fluent Bit ships both, so neither is lost — but only the first can"
	echo "be grouped, filtered by target, or joined to a trace."
	echo
	echo "**Product files emitting structured events:**"
	echo
	bullets "$structured" "**None**, which would mean the subscriber has nothing to collect."
	echo
	echo "The set below is split by \`er_obs::init::Role::is_unattended\`, because"
	echo "whether a bare \`eprintln!\` is a defect depends entirely on whether"
	echo "anyone is reading that stderr as the output of a command they typed."
	echo "Unattended roles are"
	echo "\`$(printf '%s' "$unattended_roles" | tr '\n' ' ' | sed 's/ $//')\`;"
	echo "attended ones are"
	echo "\`$(printf '%s' "$attended_roles" | tr '\n' ' ' | sed 's/ $//')\`."
	echo
	echo "Note \`Mcp\` is unattended without being **deployed** — \`er-mcp\` is in"
	echo "no Dockerfile and no chart — which is why the predicate is not called"
	echo "\`is_deployed\`."
	echo
	echo "#### The actual gap — an unattended process writing unstructured stderr"
	echo
	echo "Nobody reads these terminals. Each line here is consumed later by a log"
	echo "shipper or a host process, with no span ancestry to attribute it and no"
	echo "target to filter it by. **These are worth converting:**"
	echo
	if [[ -n ${unattended_gap//[[:space:]]/} ]]; then
		printf '%s\n' "$unattended_gap"
	else
		echo "**None.** Every unattended role emits structured events."
	fi
	echo
	echo "#### Correct as-is — attended output"
	echo
	echo "Here stderr **is the product**: \`er-entity check\`'s output is asserted"
	echo "by tests, \`er-entity lint\` prints diagnostics a human acts on, and"
	echo "\`dedupe\`'s \"wrote N rows\" is read by whoever typed the command."
	echo "Routing these through the subscriber would wrap them in JSON and put"
	echo "them behind \`RUST_LOG\`. **Do not convert them:**"
	echo
	if [[ -n ${attended_gap//[[:space:]]/} ]]; then
		printf '%s\n' "$attended_gap"
	else
		echo "**None.**"
	fi
	echo
	if [[ -n ${nosub//[[:space:]]/} ]]; then
		echo "#### Cannot emit — no subscriber installed"
		echo
		echo "These binaries never call \`er_obs::init\`, so a \`tracing\` event"
		echo "would go nowhere. \`eprintln!\` is the only channel they have:"
		echo
		printf '%s\n' "$nosub"
		echo
	fi
	echo "**Mid-conversion — both idioms in one file.** Expected and not a"
	echo "defect: an unattended role's operational diagnostics become events while"
	echo "its genuine user-facing output stays an \`eprintln!\`."
	echo
	bullets "$both" "**None.**"
}

# The source-scanning structural gates: tests that read this workspace's own
# Rust source and assert the offender list is empty.
#
# Why this set and not "every test with the word gate in its module doc": that
# wider scan returns the golden tiers and every e2e too, and a reviewer told
# `golden_em.rs` is a gate learns nothing about what not to report. The three
# textual conditions below — reads `.rs`, reads it with `read_to_string`,
# asserts `is_empty()` — select exactly the files whose finding would be a red
# test naming the offending line, which is the only property `pr-review` needs.
#
# The honest limit, stated in the reference's prose: gates that derive from a
# registry or from the handlers rather than from source text (`cardinality.rs`,
# `dispatch_coverage.rs`, `response_envelopes.rs`) are invisible here. Each is
# named in the dimension reference it belongs to instead.
gen_review_gates() {
	local f stem crate doc claim skips rows=""
	for f in rust/crates/*/tests/*.rs; do
		grep -qE '"rs"|\.rs"' "$f" || continue
		grep -q 'read_to_string' "$f" || continue
		grep -q 'is_empty()' "$f" || continue

		stem=${f#rust/crates/}
		# The module doc's first SENTENCE is the claim: every one of these files
		# opens with a one-sentence statement of what it forbids. Taking the
		# first *line* instead truncates mid-clause at the wrap column, which is
		# how `workspace_self_contained.rs` read as "no source under `rust/` may".
		doc=$(awk '
			/^\/\/!/ { sub(/^\/\/![[:space:]]?/, ""); if ($0 ~ /^[[:space:]]*$/) exit; printf "%s ", $0; next }
			{ exit }
		' "$f")
		# Cut at the first period FOLLOWED BY A SPACE, never at a bare period:
		# `lake.main.<relation>`, `pg_*.rs` and `§3.3/§3.6` are all periods
		# inside the sentence, and cutting on them produced rows reading
		# "any qualified `lake." — a table that looks maintained and says nothing.
		claim=$(printf '%s' "$doc" | sed -e 's/\. .*/./' -e 's/[[:space:]]*$//' -e 's/|/\\|/g')
		# A doc whose first sentence never ends inside the opening paragraph
		# would otherwise widen the table without bound.
		if ((${#claim} > 220)); then
			claim="${claim:0:200}…"
		fi
		[[ -n ${claim//[[:space:]]/} ]] || claim="_(no module doc — read the file)_"
		# A gate behind a tier variable self-skips in `make check`, which is
		# the one fact that changes whether a reviewer may raise its subject.
		if grep -qE 'ER_TEST_PG_DSN|DEDUPE_E2E' "$f"; then
			skips="e2e tier"
		else
			skips="\`make check\`"
		fi
		rows+="| \`$stem\` | $claim | $skips |"$'\n'
	done

	echo "| Gate | What it forbids | Runs in |"
	echo "|---|---|---|"
	if [[ -n ${rows//[[:space:]]/} ]]; then
		printf '%s' "$rows" | sort
	else
		echo "| **None found** | the detector matched nothing, which means it broke rather than that the gates were deleted | — |"
	fi
	echo
	echo "Every row above fails with the offending **file and line** in the"
	echo "message. A finding that restates one is not a false positive — it is"
	echo "worse, because it arrives with evidence and still wastes the cycle."
	echo
	echo "The \`Runs in\` column is the only reason to read a row twice. A gate in"
	echo "\`make check\` has already spoken by the time a PR is reviewable, so its"
	echo "subject is settled. A gate in the e2e tier self-skips without its tier"
	echo "variable, so on a diff whose e2e leg has not reported yet its subject is"
	echo "still open — raise it, and say which gate will confirm it."
}

# The files `regen` owns. The drift check diffs ONLY these: diffing the whole
# skills tree would report an author's in-progress prose edits as generated
# staleness, which is exactly the moment the message would be most confusing.
#
# Two of these live OUTSIDE the skills tree. `gen_skill_inventory` owns the
# inventory table in the agent-skills standard and the "Doing → Skill" table in
# CLAUDE.md, because those were the two places a human had to remember to edit
# when adding a skill — and both said "Fifteen" while the tree held more.
generated_files=(
	"$skills_dir/entity-framework/references/entities-index.md"
	"$skills_dir/rust-conventions/references/crate-map.md"
	"$skills_dir/rust-conventions/references/gates.md"
	"$skills_dir/sql-analysis/references/diagnostics.md"
	"$skills_dir/sql-analysis/references/ff-analysis.md"
	"$skills_dir/ci-and-supply-chain/references/required-checks.md"
	"$skills_dir/frontend-testing/references/tiers-and-thresholds.md"
	"$skills_dir/frontend-design-system/references/tokens-and-primitives.md"
	"$skills_dir/frontend-data-ui/references/screens.md"
	"$skills_dir/frontend-data-ui/references/dense-surfaces.md"
	"$skills_dir/aws-account-operations/references/footprint.md"
	"$skills_dir/infra-platform/references/coverage.md"
	"$skills_dir/land-pr/references/post-merge.md"
	"$skills_dir/er-engine-ops/references/stages-and-flags.md"
	"$skills_dir/observability/references/surface.md"
	"$skills_dir/pr-review/references/not-a-finding.md"
	"$skills_dir/skill-writer/references/agent-skills.md"
	"CLAUDE.md"
)

run_regen() {
	echo "skills-check: regen"
	local tmp
	tmp=$(mktemp)

	gen_entities_index >"$tmp"
	splice "$skills_dir/entity-framework/references/entities-index.md" entities-index "$tmp"

	gen_crate_map >"$tmp"
	splice "$skills_dir/rust-conventions/references/crate-map.md" crate-map "$tmp"

	gen_rust_gates >"$tmp"
	splice "$skills_dir/rust-conventions/references/gates.md" rust-gates "$tmp"

	gen_diagnostics >"$tmp"
	splice "$skills_dir/sql-analysis/references/diagnostics.md" diagnostics "$tmp"

	gen_sql_planner_pins >"$tmp"
	splice "$skills_dir/sql-analysis/references/ff-analysis.md" sql-planner-pins "$tmp"

	gen_required_checks >"$tmp"
	splice "$skills_dir/ci-and-supply-chain/references/required-checks.md" required-checks "$tmp"

	gen_tiers_and_thresholds >"$tmp"
	splice "$skills_dir/frontend-testing/references/tiers-and-thresholds.md" tiers-and-thresholds "$tmp"

	gen_design_tokens >"$tmp"
	splice "$skills_dir/frontend-design-system/references/tokens-and-primitives.md" design-tokens "$tmp"

	gen_screens_coverage >"$tmp"
	splice "$skills_dir/frontend-data-ui/references/screens.md" screens-coverage "$tmp"

	gen_dense_surface_facts >"$tmp"
	splice "$skills_dir/frontend-data-ui/references/dense-surfaces.md" dense-surface-facts "$tmp"

	gen_aws_footprint >"$tmp"
	splice "$skills_dir/aws-account-operations/references/footprint.md" aws-footprint "$tmp"

	gen_infra_coverage >"$tmp"
	splice "$skills_dir/infra-platform/references/coverage.md" infra-coverage "$tmp"

	gen_post_merge_surface >"$tmp"
	splice "$skills_dir/land-pr/references/post-merge.md" post-merge-surface "$tmp"

	gen_er_command_surface >"$tmp"
	splice "$skills_dir/er-engine-ops/references/stages-and-flags.md" er-command-surface "$tmp"

	gen_obs_surface >"$tmp"
	splice "$skills_dir/observability/references/surface.md" obs-surface "$tmp"

	gen_review_gates >"$tmp"
	splice "$skills_dir/pr-review/references/not-a-finding.md" review-gates "$tmp"

	# Two projections of one field, so the two documents cannot disagree about
	# which skills exist even though they show different columns.
	gen_skill_inventory covers >"$tmp"
	splice "$skills_dir/skill-writer/references/agent-skills.md" skill-inventory "$tmp"

	gen_description_budget >"$tmp"
	splice "$skills_dir/skill-writer/references/agent-skills.md" description-budget "$tmp"

	gen_skill_inventory doing >"$tmp"
	splice "CLAUDE.md" skill-inventory "$tmp"

	rm -f "$tmp"
	note "regenerated ${#generated_files[@]} generated files"
}

# Everything between `<!-- BEGIN GENERATED: … -->` and its END marker, for
# every block in the file, with the markers included so a block that disappears
# is also a difference. Reads stdin.
generated_blocks() {
	awk '
		/<!-- BEGIN GENERATED: /  { inblock = 1 }
		inblock                   { print }
		/<!-- END GENERATED: /    { inblock = 0 }
	'
}

run_drift_check() {
	# Compare only the GENERATED BLOCKS, not the whole files.
	#
	# This used to be `git diff --quiet HEAD -- "${generated_files[@]}"`, which
	# was right while every target was a generated-only reference. It stopped
	# being right when `CLAUDE.md` and the agent-skills standard joined the array:
	# they are mostly hand-written prose, so ANY in-progress edit to either —
	# a sentence in the authoring rules, a line in the gate table — failed the
	# check with "generated skill references are stale" while every generated
	# block was byte-identical to HEAD. A gate that fires on prose it does not
	# own teaches people to ignore it, which is the same failure as a gate that
	# stays green when it matters.
	#
	# Still HEAD-relative, so a `git add` of a stale file cannot launder it.
	#
	# Two liveness guards, because the per-file comparison below skips anything
	# it cannot read out of HEAD — and a loop that skips everything reports a
	# clean tree. Outside a git repository this check passed, silently, having
	# compared nothing. Same shape as `lint`'s zero-skills exit and
	# check-bff-authz.mjs's zero-routes exit: a gate that cannot tell success
	# from never-ran is worse than no gate.
	if ! git rev-parse --verify HEAD >/dev/null 2>&1; then
		red "FAIL drift check cannot resolve HEAD — not a git repository, or a repository with no commits."
		fail=1
		return
	fi

	local f stale=() compared=0 tmp_head tmp_work
	tmp_head=$(mktemp)
	tmp_work=$(mktemp)
	for f in "${generated_files[@]}"; do
		# Absent from HEAD means newly added: `git diff HEAD` would not have
		# reported it either, so keep that behaviour rather than inventing a
		# new failure for a file this run just created.
		git cat-file -e "HEAD:$f" 2>/dev/null || continue
		compared=$((compared + 1))
		git show "HEAD:$f" | generated_blocks >"$tmp_head"
		generated_blocks <"$f" >"$tmp_work"
		if ! cmp -s "$tmp_head" "$tmp_work"; then
			stale+=("$f")
		fi
	done

	if ((compared == 0)); then
		red "FAIL drift check compared 0 files — every generated_files path is untracked or wrong."
		rm -f "$tmp_head" "$tmp_work"
		fail=1
		return
	fi

	if ((${#stale[@]})); then
		red "FAIL generated skill references are stale. Run 'make skills-check' and commit the diff."
		for f in "${stale[@]}"; do
			printf '  %s\n' "$f"
			# The CONTENT, not just a filename. A bare path gives no way to tell
			# "someone forgot to commit the regen" from "this generator emits
			# different bytes on your machine than on CI" — the second being
			# invisible locally, because locally it always agrees with itself.
			# Telling those apart from a `126 ++---` line alone once cost an hour.
			git show "HEAD:$f" | generated_blocks >"$tmp_head"
			generated_blocks <"$f" >"$tmp_work"
			#
			# `|| true` is load-bearing. `diff` exits 1 when files differ —
			# always, here — and under `set -euo pipefail` that aborted the
			# script MID-LOOP, with three consequences that all hid behind an
			# exit code which happened to still be 1:
			#
			#   - only the FIRST stale file was ever named, so fixing it and
			#     re-running surfaced the next: whack-a-mole with no count;
			#   - the `fail=1` below was never reached (the status came from
			#     `set -e` instead, coincidentally the same value);
			#   - in `all` mode, `run_lint` and `run_override_check` never ran
			#     AT ALL. Drift is stale on precisely the PRs that edit a
			#     skill, which are precisely the PRs where the authoring rules
			#     and the override assertions matter. The gate went quiet
			#     exactly when it should have spoken.
			#
			# Confirmed by committing two stale generated blocks and checking
			# that both are named and both later phases still run.
			diff -u --label "HEAD/$f" --label "regenerated/$f" "$tmp_head" "$tmp_work" | head -40 || true
		done
		fail=1
	else
		note "generated references are current"
	fi
	rm -f "$tmp_head" "$tmp_work"
}

# ---------------------------------------------------------------------------

# WHICH STAGES ACTUALLY ASSERTED SOMETHING, so the verdict cannot over-claim.
#
# `regen` rewrites the generated blocks and checks NOTHING — yet every arm used
# to fall through to one unconditional `skills-check: ok`. That is a verdict the
# run had not earned, and it is read by more than one caller: a person reaching
# for the gate, and `generated-artifact-healer`, whose allow-list includes
# `skills-check.sh regen`.
#
# Measured cost of that line: a red `agent skills (lint + drift)` on #812. The
# author ran `regen`, saw `skills-check: ok`, and pushed a SKILL.md eight lines
# past the 500-line body cap — a `lint` error that `regen` never looks for. The
# gate is `make skills-check`, and the output now says so rather than implying
# the opposite.
checked=()

case "${1:-all}" in
lint)
	run_lint
	run_override_check
	run_generator_check
	checked=(lint overrides generators)
	;;
regen) run_regen ;;
overrides)
	run_override_check
	checked=(overrides)
	;;
generators)
	run_generator_check
	checked=(generators)
	;;
check)
	run_regen
	run_drift_check
	checked=(drift)
	;;
all)
	run_regen
	run_drift_check
	run_lint
	run_override_check
	run_generator_check
	checked=(drift lint overrides generators)
	;;
*)
	echo "usage: $0 [lint|regen|overrides|generators|check|all]" >&2
	exit 2
	;;
esac

if ((fail)); then
	exit 1
fi

# `${#checked[@]}` is safe on an empty array under `set -u`; `${checked[@]}` is
# NOT on bash 3.2, which is what macOS ships — hence the guard rather than a
# default expansion.
if ((${#checked[@]} == 0)); then
	echo "skills-check: regen done — NOTHING WAS CHECKED. The gate is 'make skills-check'."
else
	joined=$(printf '%s, ' "${checked[@]}")
	echo "skills-check: ok (${joined%, })"
fi
