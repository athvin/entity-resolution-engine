#!/usr/bin/env bash
# `make run`: the whole thing, one command.
#
# The control-plane container -> er-server (API + in-process dispatcher) -> a
# seeded demo tenant -> the web UI, then block until Ctrl+C, which stops every
# host process this script started. The container keeps running, exactly as
# `up.sh` leaves it; `make frontend-dev-reset` drops everything.
#
# This wraps `up.sh` rather than repeating it: that script owns the boot.
#
# One rough edge it exists to remove: the web server needs the `ERWEB_*`
# variables from `env.sh`, and the repo ships no `.env.local` -- so `pnpm dev`
# in a fresh terminal fails env validation on `ERWEB_DATABASE_URL`. Sourcing
# `env.sh` here means the child inherits them.
#
# The other rough edge is gone rather than handled. A half-up stack used to be
# silent — the Python API answered while nothing drained the queue, so every run
# sat in `queued` forever — and this script detected and named it. er-server now
# runs the dispatcher IN-PROCESS (`ER_DISPATCH_IN_PROCESS=1`), so a healthy API
# is a running dispatcher by construction and that state is unreachable.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${SCRIPT_DIR}/env.sh"

WEB_PORT="${ER_DEV_WEB_PORT:-3000}"
API_HEALTH="http://localhost:8000/healthz"
# Generous because a cold boot has to `cargo build` er-server and its bundled
# DuckDB, which is minutes the first time and seconds after.
BOOT_TIMEOUT=900
UP_PID=""
WEB_PID=""

log() { printf '==> %s\n' "$*"; }
die() {
    printf 'run: %s\n' "$*" >&2
    exit 1
}

api_healthy() { curl -sf -m 3 "${API_HEALTH}" >/dev/null 2>&1; }
port_busy() { lsof -ti "tcp:$1" -s TCP:LISTEN >/dev/null 2>&1; }

cleanup() {
    # Reverse order, and only what this script owns: an er-server that was
    # already running when we started is left exactly as it was found.
    if [[ -n "${WEB_PID}" ]]; then
        log "stopping the web server"
        kill "${WEB_PID}" 2>/dev/null || true
        wait "${WEB_PID}" 2>/dev/null || true
    fi
    if [[ -n "${UP_PID}" ]]; then
        log "stopping er-server (the container stays up)"
        # up.sh traps TERM and kills the er-server it started.
        kill "${UP_PID}" 2>/dev/null || true
        wait "${UP_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

[[ -d "${REPO_ROOT}/frontend/node_modules" ]] ||
    die "frontend/node_modules is missing; run: cd frontend && corepack pnpm install --frozen-lockfile"

mkdir -p "${STATE_DIR}/logs"

# ----------------------------------------------------------------- er-server
if api_healthy; then
    log "er-server already healthy on :8000 — reusing it (dispatcher is in-process)"
elif port_busy 8000; then
    die "port 8000 is held by something that is not a healthy er-server.
    Stop the stray and re-run:  pkill -f 'er-server'"
else
    log "starting the container + er-server (log: frontend/dev/.state/logs/up.log)"
    bash "${SCRIPT_DIR}/up.sh" >"${STATE_DIR}/logs/up.log" 2>&1 &
    UP_PID=$!
    for _ in $(seq 1 "${BOOT_TIMEOUT}"); do
        api_healthy && break
        # Fail on the boot's own terms rather than after the full timeout: a
        # cold cargo build is slow, but a dead up.sh never recovers.
        kill -0 "${UP_PID}" 2>/dev/null ||
            die "er-server failed to start; see ${STATE_DIR}/logs/up.log"
        sleep 1
    done
    api_healthy || die "er-server did not become healthy in ${BOOT_TIMEOUT}s; see ${STATE_DIR}/logs/up.log"
    log "er-server healthy on :8000"
fi

# ---------------------------------------------------------------------- seed
if [[ -f "${STATE_DIR}/keys.json" ]]; then
    log "demo tenant already seeded — 'make frontend-dev-reset' to start clean"
else
    log "seeding the demo tenant: org, keys, config v1, base_10 resolved, logins"
    # Through make, so the seed command has one definition.
    "${MAKE:-make}" -C "${REPO_ROOT}" frontend-seed
fi

# ----------------------------------------------------------------------- web
web_serving_us() { curl -sf -m 5 "http://localhost:${WEB_PORT}/login" >/dev/null 2>&1; }

if web_serving_us; then
    # Adopted, not refused, for the same reason a healthy er-server is: the point
    # of one command is that running it twice is safe. Left alone on exit, since
    # this invocation does not own it.
    log "a dev server is already serving :${WEB_PORT} — reusing it"
elif port_busy "${WEB_PORT}"; then
    die "port ${WEB_PORT} is held by something that is not this app; stop it or set ER_DEV_WEB_PORT"
else
    log "starting the web UI on :${WEB_PORT}"
    # `next` directly rather than `pnpm dev`, whose port is hardcoded; the env
    # this shell sourced is what makes the BFF's env validation pass.
    (cd "${REPO_ROOT}/frontend" && exec node_modules/.bin/next dev --port "${WEB_PORT}") &
    WEB_PID=$!

    for _ in $(seq 1 60); do
        web_serving_us && break
        kill -0 "${WEB_PID}" 2>/dev/null || die "the web server exited; see the output above"
        sleep 1
    done
    web_serving_us || die "the web server did not answer on :${WEB_PORT}; see the output above"
fi

# The last line states what THIS invocation owns, because that is what Ctrl+C
# acts on. A run that adopted everything owns nothing and returns immediately,
# and promising it stops things it never started would be a lie about which
# terminal holds the stack.
if [[ -n "${WEB_PID}" && -n "${UP_PID}" ]]; then
    OWNS="Ctrl+C stops the web server and er-server; the container keeps running."
elif [[ -n "${WEB_PID}" ]]; then
    OWNS="Ctrl+C stops the web server; the er-server it adopted keeps running."
elif [[ -n "${UP_PID}" ]]; then
    OWNS="Ctrl+C stops er-server; the web server it adopted keeps running."
else
    OWNS="Already up before this ran, so this command owns nothing and exits now."
fi

cat <<BANNER

==> ready
    web:    http://localhost:${WEB_PORT}
    api:    http://localhost:8000  (operator token: ${ER_OPERATOR_TOKEN})
    logins: admin@acme.dev / steward@acme.dev / viewer@acme.dev  password-123!
            root@er.dev (super admin), admin@dupezone.com / asdfasdf
    logs:   frontend/dev/.state/logs/
    ${OWNS}

BANNER

# Hold the terminal open on whatever this invocation actually started, so Ctrl+C
# reaches the cleanup trap. `|| true` because waiting on a process we just killed
# exits non-zero, and a deliberate Ctrl+C is not a failed build.
if [[ -n "${WEB_PID}" ]]; then
    wait "${WEB_PID}" || true
elif [[ -n "${UP_PID}" ]]; then
    wait "${UP_PID}" || true
fi
