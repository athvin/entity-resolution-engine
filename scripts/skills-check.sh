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
		e2e) cmd='`make frontend-e2e` — release gate, needs Docker (~10 min)' ;;
		*) cmd='—' ;;
		esac
		printf '| %s | `frontend/tests/%s/` | %s |\n' "$t" "$t" "$cmd"
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
			| sed -e 's/^path: //' -e 's/^[`"'"'"']//' -e 's/[`"'"'"']$//' -e 's#\${ORG}#[org]#g' \
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

# The inventory table that docs/agent-skills.md and CLAUDE.md each carry.
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
#
# Two of these live OUTSIDE the skills tree. `gen_skill_inventory` owns the
# inventory table in docs/agent-skills.md and the "Doing → Skill" table in
# CLAUDE.md, because those were the two places a human had to remember to edit
# when adding a skill — and both said "Fifteen" while the tree held more.
generated_files=(
	"$skills_dir/entity-framework/references/entities-index.md"
	"$skills_dir/rust-conventions/references/crate-map.md"
	"$skills_dir/sql-analysis/references/diagnostics.md"
	"$skills_dir/ci-and-supply-chain/references/required-checks.md"
	"$skills_dir/frontend-testing/references/tiers-and-thresholds.md"
	"$skills_dir/frontend-design-system/references/tokens-and-primitives.md"
	"$skills_dir/frontend-data-ui/references/screens.md"
	"$skills_dir/aws-account-operations/references/footprint.md"
	"docs/agent-skills.md"
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

	gen_diagnostics >"$tmp"
	splice "$skills_dir/sql-analysis/references/diagnostics.md" diagnostics "$tmp"

	gen_required_checks >"$tmp"
	splice "$skills_dir/ci-and-supply-chain/references/required-checks.md" required-checks "$tmp"

	gen_tiers_and_thresholds >"$tmp"
	splice "$skills_dir/frontend-testing/references/tiers-and-thresholds.md" tiers-and-thresholds "$tmp"

	gen_design_tokens >"$tmp"
	splice "$skills_dir/frontend-design-system/references/tokens-and-primitives.md" design-tokens "$tmp"

	gen_screens_coverage >"$tmp"
	splice "$skills_dir/frontend-data-ui/references/screens.md" screens-coverage "$tmp"

	gen_aws_footprint >"$tmp"
	splice "$skills_dir/aws-account-operations/references/footprint.md" aws-footprint "$tmp"

	# Two projections of one field, so the two documents cannot disagree about
	# which skills exist even though they show different columns.
	gen_skill_inventory covers >"$tmp"
	splice "docs/agent-skills.md" skill-inventory "$tmp"

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
	# being right when `CLAUDE.md` and `docs/agent-skills.md` joined the array:
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
			diff -u --label "HEAD/$f" --label "regenerated/$f" "$tmp_head" "$tmp_work" | head -40
		done
		fail=1
	else
		note "generated references are current"
	fi
	rm -f "$tmp_head" "$tmp_work"
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
