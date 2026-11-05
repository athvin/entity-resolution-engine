#!/usr/bin/env bash
# Build a fresh fake repo and run one merge-bus scenario end to end.
#   scenario.sh <scenario>
# Prints the sim directory on the last line.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WF="${WF:?WF must point at merge-bus.yml}"
SCEN="$1"

SIM="$(mktemp -d)"; export SIM

git init --quiet --bare "$SIM/origin.git"
git init --quiet "$SIM/seed"
cd "$SIM/seed"
git config user.name s; git config user.email s@x
mkdir -p src; echo base >src/base.txt
git add -A; git commit --quiet -m base
git push --quiet "$SIM/origin.git" HEAD:refs/heads/main
BASE="$(git rev-parse HEAD)"

mk() { git switch --quiet --detach "$BASE"; echo "$3" >"src/$2"; git add -A
       git commit --quiet -m "$1"; git push --quiet "$SIM/origin.git" "HEAD:refs/heads/$1"; }
mk feat-a a.txt alpha
mk feat-b b.txt beta
mk feat-c c.txt gamma
git switch --quiet --detach "$BASE"; echo COLLIDES >src/a.txt
git add -A; git commit --quiet -m collider
git push --quiet "$SIM/origin.git" HEAD:refs/heads/feat-collide
# stacked on feat-a
git switch --quiet --detach "$(git rev-parse feat-a)" 2>/dev/null || git switch --quiet --detach "$BASE"
git fetch --quiet "$SIM/origin.git" feat-a; git switch --quiet --detach FETCH_HEAD
echo child >src/child.txt; git add -A; git commit --quiet -m child
git push --quiet "$SIM/origin.git" HEAD:refs/heads/feat-a-child

# GitHub synthesizes `refs/pull/<n>/head` for every PR, and the bus fetches
# that rather than the branch name (immune to a rename mid-batch). A plain bare
# repo has no such refs, so the simulation has to provide them.
pullref() { git -C "$SIM/origin.git" update-ref "refs/pull/$1/head" \
              "$(git -C "$SIM/origin.git" rev-parse "refs/heads/$2")"; }
pullref 101 feat-a
pullref 102 feat-b
pullref 104 feat-c
pullref 107 feat-collide
pullref 201 feat-a-child

git clone --quiet "$SIM/origin.git" "$SIM/landing"
git -C "$SIM/landing" config user.name s; git -C "$SIM/landing" config user.email s@x

cat >"$SIM/state" <<EOF
bus_number=900
steps_per_run=42
recent_run_ids=1 2 3 4 5
bus_run_ids=7
merge_fails_on=
merge_noops_on=
failed_contexts=
failed_contexts_names=
held_contexts=
approve_works=
bus_state_final=CLEAN
move_main=0
api_fails_recent=
api_fails_bus=
files_101=src/a.txt
files_102=src/b.txt
candidates=101 102
state_101=CLEAN
draft_101=false
base_101=main
head_101=feat-a
state_102=CLEAN
draft_102=false
base_102=main
head_102=feat-b
EOF
: >"$SIM/calls.log"; : >"$SIM/busstate.seq"
add() { printf '%s\n' "$@" >>"$SIM/state"; }

case "$SCEN" in
  green)     printf 'BLOCKED\nCLEAN\n' >"$SIM/busstate.seq" ;;
  stacked)   printf 'CLEAN\n'          >"$SIM/busstate.seq"; add "children_feat-a=201" "head_201=feat-a-child" ;;
  red)       printf 'BLOCKED\n'        >"$SIM/busstate.seq"
             add "failed_contexts_names=test entities" "bus_state_final=BLOCKED" ;;
  starved)   printf 'BLOCKED\n'        >"$SIM/busstate.seq"
             add "failed_contexts_names=test" "bus_state_final=BLOCKED" "steps_run_7=0" ;;
  preflight) add "steps_per_run=0" ;;
  moved)     printf 'CLEAN\n'          >"$SIM/busstate.seq"; add "move_main=1" ;;
  partial)   printf 'CLEAN\n'          >"$SIM/busstate.seq"; add "merge_fails_on=102" ;;
  lying)     printf 'CLEAN\n'          >"$SIM/busstate.seq"; add "merge_noops_on=102" ;;
  preflight-blind)
             printf 'BLOCKED\nCLEAN\n'  >"$SIM/busstate.seq"; add "api_fails_recent=1" ;;
  workflow-touching)
             printf 'BLOCKED\nCLEAN\n'  >"$SIM/busstate.seq"
             add "candidates=101 108 102" \
                 "state_108=CLEAN" "draft_108=false" "base_108=main" "head_108=feat-c" \
                 "files_108=.github/workflows/rust.yml src/c.txt" ;;
  detect-died)
             printf 'BLOCKED\nCLEAN\n'  >"$SIM/busstate.seq"
             add "candidates=101 109 102" \
                 "state_109=CLEAN" "draft_109=false" "base_109=main" "head_109=feat-c" \
                 "files_109=src/c.txt" "detect_failed_oid109=1" ;;
  detect-read-flaky)
             printf 'BLOCKED\nCLEAN\n'  >"$SIM/busstate.seq"
             add "check_flaky_oid101=1" "check_flaky_oid102=1" ;;
  held-released)
             printf 'BLOCKED\nBLOCKED\nCLEAN\n' >"$SIM/busstate.seq"
             add "held_contexts=test entities" "approve_works=1" "steps_run_7=0" ;;
  held-stuck)
             printf 'BLOCKED\nBLOCKED\n'        >"$SIM/busstate.seq"
             add "held_contexts=test entities" "approve_works=0" "bus_state_final=BLOCKED" "steps_run_7=0" ;;
  verdict-blind)
             printf 'BLOCKED\n'        >"$SIM/busstate.seq"
             add "failed_contexts_names=test" "bus_state_final=BLOCKED" "api_fails_bus=1" ;;
  reject)    printf 'CLEAN\n'          >"$SIM/busstate.seq"
             add "candidates=101 102 104 105 106" \
                 "state_104=DIRTY" "draft_104=false" "base_104=main" "head_104=feat-c" \
                 "state_105=CLEAN" "draft_105=false" "base_105=feat-a" "head_105=feat-x" \
                 "state_106=CLEAN" "draft_106=true"  "base_106=main" "head_106=feat-y" ;;
  collide)   printf 'CLEAN\n'          >"$SIM/busstate.seq"
             add "candidates=101 107 102" \
                 "state_107=CLEAN" "draft_107=false" "base_107=main" "head_107=feat-collide" ;;
  *) echo "unknown scenario $SCEN" >&2; exit 2 ;;
esac

set +e
python3 "$HERE/run.py" "$WF" "$SIM" dry_run=false >"$SIM/log" 2>&1
echo "$?" >"$SIM/rc"
set -e
echo "$SIM"
