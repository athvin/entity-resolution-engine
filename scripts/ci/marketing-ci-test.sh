#!/usr/bin/env bash
# Exercise the actual event detectors and tier runner in an isolated repository.
# No GitHub calls, node installation, Rust build or database required.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
. scripts/ci/frontend-paths.sh
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
pass=0
expect() {
  if [ "$2" != "$3" ]; then
    printf 'FAIL %s\nwant: %s\ngot: %s\n' "$1" "$2" "$3" >&2
    exit 1
  fi
  pass=$((pass + 1))
  printf 'ok %s\n' "$1"
}
classify() { printf '%s\n' "$@" | frontend_classify | tr '\n' ' '; }
site='frontend=true marketing_only=true '
app='frontend=true marketing_only=false '
expect 'empty frontend diff' 'frontend=false marketing_only=false ' "$(classify)"
expect 'backend only' 'frontend=false marketing_only=false ' "$(classify rust/Cargo.toml)"
expect 'marketing page' "$site" "$(classify 'frontend/src/app/(marketing)/page.tsx')"
expect 'marketing plus backend' "$site" "$(classify frontend/src/components/marketing/hero.tsx rust/Cargo.toml)"
for path in frontend/src/app/layout.tsx frontend/src/app/globals.css frontend/src/proxy.ts frontend/package.json frontend/pnpm-lock.yaml frontend/playwright.config.ts frontend/tests/functional/helpers.ts frontend/src/lib/api/schema.d.ts scripts/ci/marketing-paths.sh scripts/ci/marketing-ci-test.sh scripts/ci/frontend-run.sh .github/workflows/frontend.yml; do
  expect "full app with $path" "$app" "$(classify frontend/src/components/marketing/hero.tsx "$path")"
done

# Actual git diffs: event ranges, rename/deletion handling and fail-safe inputs.
mkdir -p "$tmp/repo/scripts/ci"
cp scripts/ci/{frontend-detect,frontend-paths,marketing-paths,rust-detect,detect-paths}.sh "$tmp/repo/scripts/ci/"
git -C "$tmp/repo" init -q
git -C "$tmp/repo" config user.name 'CI fixture'
git -C "$tmp/repo" config user.email 'fixture@example.invalid'
mkdir -p "$tmp/repo/frontend/src/components/marketing"
printf 'shared\n' > "$tmp/repo/frontend/src/components/shell.tsx"
printf 'hero\n' > "$tmp/repo/frontend/src/components/marketing/hero.tsx"
git -C "$tmp/repo" add .
git -C "$tmp/repo" commit -qm base
base="$(git -C "$tmp/repo" rev-parse HEAD)"
printf 'edited\n' >> "$tmp/repo/frontend/src/components/marketing/hero.tsx"
git -C "$tmp/repo" commit -qam marketing
head="$(git -C "$tmp/repo" rev-parse HEAD)"
detect() {
  : > "$tmp/out"
  EVENT="$2" BASE_SHA="$base" HEAD_SHA="$head" MQ_BASE="$base" MQ_HEAD="$head" \
    PUSH_BEFORE="${3-$base}" PUSH_AFTER="$head" FULL_CI="${4-false}" \
    GITHUB_OUTPUT="$tmp/out" bash "$tmp/repo/scripts/ci/$1-detect.sh" > "$tmp/detect.log" 2>&1 || return $?
  tr '\n' ' ' < "$tmp/out"
}
rust_all='code=true erd=true drift=true empty=false '
rust_skip='code=false erd=false drift=false empty=false '
for event in pull_request merge_group push; do
  expect "frontend $event: marketing subset" "$site" "$(detect frontend "$event")"
  expect "Rust $event: no compile" "$rust_skip" "$(detect rust "$event")"
done
for gate in rust frontend; do
  expected="$app"
  [ "$gate" != rust ] || expected="$rust_all"
  for event in schedule workflow_dispatch; do
    expect "$gate $event: full verification" "$expected" "$(detect "$gate" "$event")"
  done
  for before in '' 0000000000000000000000000000000000000000 missing-commit; do
    expect "$gate push: unknown base runs full" "$expected" "$(detect "$gate" push "$before")"
  done
  expect "$gate ci:full overrides marketing" "$expected" "$(detect "$gate" pull_request "$base" true)"
  saved_head=$head
  head=missing-commit
  if detect "$gate" pull_request > /dev/null; then
    echo "FAIL $gate accepted an invalid head" >&2
    exit 1
  fi
  head=$saved_head
  pass=$((pass + 1))
done

# A shared component moved into marketing still changed the app. Git's normal
# rename detection reports only the destination; --no-renames preserves both.
git -C "$tmp/repo" mv frontend/src/components/shell.tsx frontend/src/components/marketing/shell.tsx
git -C "$tmp/repo" commit -qm rename
head="$(git -C "$tmp/repo" rev-parse HEAD)"
expect 'rename from shared app triggers full frontend' "$app" "$(detect frontend pull_request)"
expect 'rename from shared app triggers Rust' "$rust_all" "$(detect rust pull_request)"
expect 'mixed push retains full Rust verification' "$rust_all" "$(detect rust push)"
base=$head
git -C "$tmp/repo" rm -q frontend/src/components/marketing/hero.tsx
git -C "$tmp/repo" commit -qm deletion
head="$(git -C "$tmp/repo" rev-parse HEAD)"
expect 'marketing deletion still gets browser tests' "$site" "$(detect frontend pull_request)"
expect 'marketing deletion skips Rust' "$rust_skip" "$(detect rust pull_request)"

# Command selection and failure propagation. Mock pnpm only, run the real
# wrapper. It must forward shard flags and never accept an empty test suite.
mkdir -p "$tmp/bin"
cat > "$tmp/bin/pnpm" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$COMMAND_LOG"
exit "${PNPM_EXIT:-0}"
STUB
chmod +x "$tmp/bin/pnpm"
export PATH="$tmp/bin:$PATH" COMMAND_LOG="$tmp/commands"
: > "$COMMAND_LOG"
MARKETING_ONLY=true bash scripts/ci/frontend-run.sh check
expect 'static check excludes app unit and API tests' $'lint\nformat:check\ntypecheck' "$(cat "$COMMAND_LOG")"
: > "$COMMAND_LOG"
(cd "$tmp/repo" && MARKETING_ONLY=false bash "$root/scripts/ci/frontend-run.sh" check)
expect 'app check retains coverage and API drift' $'check\ngen:api' "$(cat "$COMMAND_LOG")"
: > "$COMMAND_LOG"
MARKETING_ONLY=true bash scripts/ci/frontend-run.sh e2e --project=mobile-safari --shard=2/2 --workers=2
expect 'static tests select marketing and axe; forward shard' \
  'test:e2e (^|/)(marketing(-[a-z0-9-]+)?|a11y)\.spec\.ts$ --grep marketing(-[a-z0-9-]+)?\.spec\.ts|accessibility.*landing --project=mobile-safari --shard=2/2 --workers=2' "$(cat "$COMMAND_LOG")"
: > "$COMMAND_LOG"
MARKETING_ONLY=false bash scripts/ci/frontend-run.sh e2e --workers=2
expect 'app keeps all browser tests' 'test:e2e --workers=2' "$(cat "$COMMAND_LOG")"
for tier in check e2e; do
  code=0
  MARKETING_ONLY=true PNPM_EXIT=7 bash scripts/ci/frontend-run.sh "$tier" || code=$?
  expect "$tier propagates tool failure" 7 "$code"
done
echo "marketing-ci: $pass checks passed"
