#!/usr/bin/env bash
# `make run`: the whole thing, one command.
#
# Containers (Postgres x2 + MinIO) -> erserver API -> dispatcher -> a seeded
# demo tenant -> the web UI, then block until Ctrl+C, which stops every host
# process this script started. Containers keep running, exactly as `up.sh`
# leaves them; `make frontend-dev-reset` is still the way to drop everything.
#
# This wraps `up.sh` rather than repeating it: that script owns the boot order,
# including the one ordering that matters (API strictly before dispatcher, or
# both race `ensure_schema` on a first boot and the dispatcher's long-lived
# leader connection blocks the API's DDL indefinitely).
#
# Two rough edges it exists to remove:
#
#  * The web server needs the `ERWEB_*` variables from `env.sh`, and the repo
#    ships no `.env.local` -- so `pnpm dev` in a fresh terminal fails env
#    validation on `ERWEB_DATABASE_URL`. Sourcing `env.sh` here means the
#    child inherits them.
#  * A half-up stack is silent: the API answers while nothing runs jobs, so
#    every run sits in `queued` forever. That state is detected and named
#    rather than added to.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${SCRIPT_DIR}/env.sh"

WEB_PORT="${ER_DEV_WEB_PORT:-3000}"
API_HEALTH="http://localhost:8000/healthz"
BOOT_TIMEOUT=180
UP_PID=""
WEB_PID=""

log() { printf '==> %s\n' "$*"; }
die() {
    printf 'run: %s\n' "$*" >&2
    exit 1
}

api_healthy() { curl -sf -m 3 "${API_HEALTH}" >/dev/null 2>&1; }
dispatcher_alive() { pgrep -f "erserver[.]dispatcher" >/dev/null 2>&1; }
port_busy() { lsof -ti "tcp:$1" -s TCP:LISTEN >/dev/null 2>&1; }

cleanup() {
    # Reverse order, and only what this script owns: an erserver stack that was
    # already running when we started is left exactly as it was found.
    if [[ -n "${WEB_PID}" ]]; then
        log "stopping the web server"
        kill "${WEB_PID}" 2>/dev/null || true
        wait "${WEB_PID}" 2>/dev/null || true
    fi
    if [[ -n "${UP_PID}" ]]; then
        log "stopping erserver (containers stay up)"
        # up.sh traps TERM and kills the api + dispatcher it started.
        kill "${UP_PID}" 2>/dev/null || true
        wait "${UP_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

[[ -d "${REPO_ROOT}/frontend/node_modules" ]] ||
    die "frontend/node_modules is missing; run: cd frontend && corepack pnpm install --frozen-lockfile"

mkdir -p "${STATE_DIR}/logs"

# ------------------------------------------------------------------ erserver
if api_healthy && dispatcher_alive; then
    log "erserver already running on :8000 with a live dispatcher — reusing it"
elif api_healthy || dispatcher_alive || port_busy 8000; then
    die "the dev stack is half-up: API $(api_healthy && echo up || echo down), dispatcher $(dispatcher_alive && echo up || echo down).
    A running API with no dispatcher accepts jobs that nothing will ever execute.
    Stop the strays and re-run:  pkill -f 'erserver[.]dispatcher'; pkill -f 'erserver.api:create_app'"
else
    log "starting containers + erserver (log: frontend/dev/.state/logs/up.log)"
    bash "${SCRIPT_DIR}/up.sh" >"${STATE_DIR}/logs/up.log" 2>&1 &
    UP_PID=$!
    for _ in $(seq 1 "${BOOT_TIMEOUT}"); do
        api_healthy && break
        # Fail on the boot's own terms rather than after the full timeout: the
        # first `uv run` of a session can be slow, but a dead up.sh never recovers.
        kill -0 "${UP_PID}" 2>/dev/null ||
            die "erserver failed to start; see ${STATE_DIR}/logs/up.log"
        sleep 1
    done
    api_healthy || die "erserver did not become healthy in ${BOOT_TIMEOUT}s; see ${STATE_DIR}/logs/up.log"
    log "erserver healthy on :8000"
fi

# ---------------------------------------------------------------------- seed
if [[ -f "${STATE_DIR}/keys.json" ]]; then
    log "demo tenant already seeded — 'make frontend-dev-reset' to start clean"
else
    log "seeding the demo tenant: corpus, train, full run, config v1, logins (a few minutes)"
    # Through make, so the seed command has one definition.
    "${MAKE:-make}" -C "${REPO_ROOT}" frontend-seed
fi

# ----------------------------------------------------------------------- web
web_serving_us() { curl -sf -m 5 "http://localhost:${WEB_PORT}/login" >/dev/null 2>&1; }

if web_serving_us; then
    # Adopted, not refused, for the same reason a healthy erserver is: the point
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
    OWNS="Ctrl+C stops the web server and erserver; containers keep running."
elif [[ -n "${WEB_PID}" ]]; then
    OWNS="Ctrl+C stops the web server; the erserver it adopted keeps running."
elif [[ -n "${UP_PID}" ]]; then
    OWNS="Ctrl+C stops erserver; the web server it adopted keeps running."
else
    OWNS="Already up before this ran, so this command owns nothing and exits now."
fi

cat <<BANNER

==> ready
    web:    http://localhost:${WEB_PORT}
    api:    http://localhost:8000  (operator token: ${ERSERVER_OPERATOR_TOKEN})
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
