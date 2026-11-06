#!/usr/bin/env bash
# devcontainer.json's `postStartCommand` — runs on EVERY start, not only on
# creation, because there is no init system in the container and so nothing else
# would bring Postgres back after a stop.
#
# Under `scripts/` rather than `.devcontainer/` for the reason
# scripts/devcontainer-postcreate.sh gives: scripts-home-check.sh.
#
# NO `sudo`, ANYWHERE, AND THAT IS THE POINT. The first version of this script
# started Debian's packaged `main` cluster with `sudo pg_ctlcluster`, which works
# in a plain `docker run` — the base image ships a NOPASSWD rule for `vscode`
# and `sudo -n true` succeeds — and FAILED in `devcontainers/ci` with
# `sudo: a password is required`. The container the action builds is not the
# container a laptop builds: it updates the remote user's uid/gid to match the
# runner, and the sudo rule did not survive that.
#
# So this owns its own cluster, in $HOME, created and run as whoever we are.
# That removes the privilege question rather than answering it, and incidentally
# survives the uid rewrite — `$HOME` is the one path the CLI's uid update is
# guaranteed to chown.
#
# initdb at RUNTIME rather than in the Dockerfile, for the same reason: a data
# directory baked at build time carries the build-time uid, and Postgres refuses
# to start on a directory it does not own. Doing it here costs ~2s on first
# start and cannot be wrong.
#
# Idempotent throughout — this runs again every time the container restarts.
#
# Run (inside the container): bash scripts/devcontainer-poststart.sh
set -euo pipefail

# Server binaries are not on PATH: `postgresql-client-16` wraps the client tools
# into /usr/bin via pg_wrapper, but `initdb` and `pg_ctl` are server binaries and
# are not wrapped. Named explicitly rather than prepended to PATH, so `psql`
# keeps resolving to the wrapper the smoke test checks for.
PGBIN=/usr/lib/postgresql/16/bin
PGDATA="$HOME/.er-postgres/data"
PGLOG="$HOME/.er-postgres/server.log"
PGPORT=5432

if [ ! -s "$PGDATA/PG_VERSION" ]; then
	echo "devcontainer-poststart: initialising a postgres 16 cluster in $PGDATA"
	mkdir -p "$(dirname "$PGDATA")"
	# `--auth=trust` because this cluster listens on localhost inside one
	# developer's container and holds only test fixtures. ER_TEST_PG_DSN still
	# carries `password=er`, which trust accepts and ignores — so the DSN stays
	# byte-identical to the one rust.yml hands its `postgres:16-alpine`
	# service, and `make check-e2e` needs no local variation.
	#
	# `-U er` makes the role the DSN names the bootstrap superuser, so there is
	# no second role to create and no `CREATE ROLE` to guard.
	"$PGBIN/initdb" --pgdata "$PGDATA" --username=er --auth=trust --encoding=UTF8 > /dev/null
fi

if ! pg_isready --quiet --host 127.0.0.1 --port "$PGPORT"; then
	echo "devcontainer-poststart: starting postgres on port $PGPORT"
	# `-k` IS REQUIRED, and omitting it is a confusing failure. The compiled-in
	# `unix_socket_directories` is /var/run/postgresql, which the package
	# creates owned by `postgres` — so a server run as any other user dies with
	#
	#   FATAL: could not create lock file
	#          "/var/run/postgresql/.s.PGSQL.5432.lock": Permission denied
	#
	# while `pg_ctl` reports only "could not start server. Examine the log
	# output", and the log it means is this $PGLOG, not anything on stderr.
	# Pointing the socket at $HOME keeps the no-privilege property of the rest
	# of this script. Connections here are TCP on 127.0.0.1 (ER_TEST_PG_DSN
	# says `host=localhost`), so the socket is only needed for a bare `psql`.
	#
	# `-w` waits for the server to accept connections, which is what makes the
	# createdb below safe: `pg_ctl start` without it returns before the server
	# is listening, and the next command fails intermittently with "could not
	# connect" — a flake that reads as a broken image.
	"$PGBIN/pg_ctl" --pgdata "$PGDATA" --log "$PGLOG" \
		--options "-p $PGPORT -k $HOME/.er-postgres" --wait start
fi

# The database ER_TEST_PG_DSN names. Postgres has no `CREATE DATABASE IF NOT
# EXISTS`, so this asks first; `createdb` on an existing database is an error,
# not a no-op, and this script runs on every start.
#
# Command substitution rather than `psql … | grep -q`: `grep -q` closes the pipe
# on its first match, which SIGPIPEs psql and makes the pipeline exit 141 under
# `pipefail`. A database that DOES exist would intermittently report as missing
# and this script would then run `createdb` and fail. The repo has been bitten
# by that exact shape in `skills-check.sh` and `tf-helpers-test.sh`; see the
# header of scripts/ci/detect-paths.sh.
exists=$(psql --host 127.0.0.1 --port "$PGPORT" --username er --dbname postgres \
	--tuples-only --no-align --quiet \
	--command "SELECT 1 FROM pg_database WHERE datname = 'er'")
if [ "$exists" != "1" ]; then
	echo "devcontainer-poststart: creating database 'er'"
	createdb --host 127.0.0.1 --port "$PGPORT" --username er er
fi

echo "devcontainer-poststart: postgres ready — ${ER_TEST_PG_DSN:-<ER_TEST_PG_DSN unset>}"
