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
# Usage: skills-check.sh [lint|regen|check|all]
#   check = regen + fail on drift (what CI runs)
#   all   = lint + check (what `make skills-check` runs)

set -euo pipefail

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
		# First person in a description is injected into the system prompt and
		# measurably hurts discovery.
		if printf '%s' "$desc" | grep -qiE '(^| )(I |I'\''m|I can|we can|you can use this)'; then
			err "$slug: description must be third person (no 'I'/'you can use this')"
		fi
	fi

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
	local f target
	while IFS= read -r f; do
		[[ -n $f ]] || continue
		while IFS= read -r target; do
			[[ -n $target ]] || continue
			case $target in
			*\\*) err "$slug: ${f#"$dir/"} links with a backslash path ('$target')" ;;
			esac
			if [[ ! -e $(dirname "$f")/$target ]]; then
				err "$slug: ${f#"$dir/"} links to a missing file ('$target')"
			fi
		done < <(relative_links "$f")
	done < <(find "$dir" -name '*.md' -type f | sort)

	# --- reference depth -----------------------------------------------------
	# Claude partially reads files reached through another reference, so every
	# reference must hang directly off SKILL.md. A reference linking to a
	# sibling reference is the shape that breaks.
	if [[ -d $dir/references ]]; then
		while IFS= read -r f; do
			while IFS= read -r target; do
				[[ -n $target ]] || continue
				case $target in
				references/* | ./references/*)
					err "$slug: ${f#"$dir/"} links to a sibling reference ('$target') — references must be one level deep from SKILL.md"
					;;
				esac
			done < <(relative_links "$f")
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
	note "examined $found skills"
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
}

# The invariant column for one E0xx code, out of the design doc's table.
# Long entries are clipped at a word boundary — these are signposts to the full
# rule, not a replacement for it.
rule_desc() {
	grep -E "^\| $1 \|" docs/entity-framework-design.md |
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
	documented=$(grep -oE '^\| E0[0-9]{2}' docs/entity-framework-design.md | sed 's/^| //' | sort -u)
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
		echo "This gap is the stated reason \`docs/entity-lint-rules.md\` is generated"
		echo "and byte-compared: a documented-only rule is exactly the drift that"
		echo "produced it."
	else
		echo "Every documented code is implemented."
	fi
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
	local wf
	for wf in .github/workflows/*.yml; do
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
			/^  [a-z][a-z0-9_-]*:[[:space:]]*$/ {
				if (job != "") emit()
				job = $1; sub(/:$/, "", job)
				name = ""; tmo = "—"; cond = "always"
			}
			/^    name:/ { sub(/^    name: */, ""); name = $0 }
			/^    timeout-minutes:/ { sub(/^    timeout-minutes: */, ""); tmo = $0 }
			/^    if:/ {
				sub(/^    if: */, "")
				cond = $0
				gsub(/\|/, "\\|", cond)
				if (length(cond) > 90) cond = substr(cond, 1, 90) " …"
			}
			END { if (job != "") emit() }
			function emit() {
				if (name == "") name = job
				printf "| `%s` | %s | %s | `%s` |\n", job, name, tmo, cond
			}
		' "$wf"
		echo
	done
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
	local t cmd
	for t in unit component functional e2e; do
		[[ -d frontend/tests/$t ]] || continue
		case $t in
		unit | component) cmd='`pnpm test`' ;;
		functional) cmd='`pnpm test:e2e`' ;;
		e2e) cmd='`make frontend-e2e` — dormant, its seed script was deleted' ;;
		*) cmd='—' ;;
		esac
		printf '| %s | `frontend/tests/%s/` | %s |\n' "$t" "$t" "$cmd"
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

	local tmp
	tmp=$(mktemp)
	awk -v start="$start" -v end="$end" -v payload="$payload" '
		index($0, start) { print; while ((getline line < payload) > 0) print line; close(payload); skip = 1; next }
		index($0, end)   { skip = 0; print; next }
		!skip            { print }
	' "$target" >"$tmp"
	mv "$tmp" "$target"
}

# The files `regen` owns. The drift check diffs ONLY these: diffing the whole
# skills tree would report an author's in-progress prose edits as generated
# staleness, which is exactly the moment the message would be most confusing.
generated_files=(
	"$skills_dir/entity-framework/references/entities-index.md"
	"$skills_dir/rust-conventions/references/crate-map.md"
	"$skills_dir/sql-analysis/references/diagnostics.md"
	"$skills_dir/ci-and-supply-chain/references/required-checks.md"
	"$skills_dir/frontend-testing/references/tiers-and-thresholds.md"
)

run_regen() {
	echo "skills-check: regen"
	local tmp
	tmp=$(mktemp)

	gen_entities_index >"$tmp"
	splice "$skills_dir/entity-framework/references/entities-index.md" entities-index "$tmp"

	gen_crate_map >"$tmp"
	splice "$skills_dir/rust-conventions/references/crate-map.md" crate-map "$tmp"

	gen_diagnostics >"$tmp"
	splice "$skills_dir/sql-analysis/references/diagnostics.md" diagnostics "$tmp"

	gen_required_checks >"$tmp"
	splice "$skills_dir/ci-and-supply-chain/references/required-checks.md" required-checks "$tmp"

	gen_tiers_and_thresholds >"$tmp"
	splice "$skills_dir/frontend-testing/references/tiers-and-thresholds.md" tiers-and-thresholds "$tmp"

	rm -f "$tmp"
	note "regenerated 5 reference files"
}

run_drift_check() {
	# HEAD-relative, so it catches staleness in both the worktree and the index
	# — a `git add` of a stale file must not be able to launder it past the gate.
	if ! git diff --quiet HEAD -- "${generated_files[@]}"; then
		red "FAIL generated skill references are stale. Run 'make skills-check' and commit the diff."
		git --no-pager diff --stat HEAD -- "${generated_files[@]}"
		fail=1
	else
		note "generated references are current"
	fi
}

# ---------------------------------------------------------------------------

case "${1:-all}" in
lint) run_lint ;;
regen) run_regen ;;
check)
	run_regen
	run_drift_check
	;;
all)
	run_regen
	run_drift_check
	run_lint
	;;
*)
	echo "usage: $0 [lint|regen|check|all]" >&2
	exit 2
	;;
esac

if ((fail)); then
	exit 1
fi
echo "skills-check: ok"
