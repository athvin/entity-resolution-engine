#!/usr/bin/env bash
# devcontainer.json's `postStartCommand` — runs on EVERY start, not only on
# creation, because there is no init system in the container and so nothing else
# would bring Postgres back after a stop.
#
# Under `scripts/` rather than `.devcontainer/` for the reason
# scripts/devcontainer-postcreate.sh gives: scripts-home-check.sh.
#
# Idempotent throughout — this runs again every time the container restarts.
#
# Run (inside the container): bash scripts/devcontainer-poststart.sh
set -euo pipefail

CLUSTER_VERSION=16
CLUSTER_NAME=main

# `pg_ctlcluster` exits 2 when the cluster is already running, which is success
# here, so its status is read rather than its exit code.
if ! pg_isready --quiet --host 127.0.0.1 --port 5432; then
	echo "devcontainer-poststart: starting postgres $CLUSTER_VERSION/$CLUSTER_NAME"
	sudo pg_ctlcluster "$CLUSTER_VERSION" "$CLUSTER_NAME" start
fi

# Wait for it rather than racing it: `pg_ctlcluster start` returns before the
# server is accepting connections, so the role creation below would intermittently
# fail with "could not connect" — the kind of flake that reads as a broken image.
for _ in $(seq 1 30); do
	pg_isready --quiet --host 127.0.0.1 --port 5432 && break
	sleep 1
done
if ! pg_isready --quiet --host 127.0.0.1 --port 5432; then
	echo "devcontainer-poststart: postgres did not accept connections within 30s" >&2
	sudo pg_ctlcluster "$CLUSTER_VERSION" "$CLUSTER_NAME" status || true
	exit 1
fi

# The role and database ER_TEST_PG_DSN names (devcontainer.json `containerEnv`),
# matching the DSN rust.yml hands its `postgres:16-alpine` service so the same
# `make check-e2e` invocation works in both places.
#
# `SELECT 1 FROM … \gexec` rather than `CREATE … IF NOT EXISTS`, which Postgres
# has for neither roles nor databases. Both guarded, so a restart is a no-op.
sudo -u postgres psql -v ON_ERROR_STOP=1 --quiet <<'SQL'
SELECT 'CREATE ROLE er LOGIN SUPERUSER PASSWORD ''er'''
 WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'er')\gexec
SELECT 'CREATE DATABASE er OWNER er'
 WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'er')\gexec
SQL

echo "devcontainer-poststart: postgres ready — ${ER_TEST_PG_DSN:-<ER_TEST_PG_DSN unset>}"
