"""Suspend this namespace when it is idle — infrastructure.md §5.2.

Runs as a CronJob (templates/idle-suspend.yaml) inside the namespace it
suspends. The predicate, verbatim from §5.2: suspend when `max(last_seen_at)`
across `erweb.sessions` is older than four hours AND no job is queued or
running — the second clause is what keeps this CronJob from burning a
`--resume` attempt under a live run. `last_seen_at` is slid on activity but
throttled to at-most-hourly updates (frontend/src/lib/auth/session.ts), which
is why the threshold is hours, not minutes.

The suspend itself is exactly `make dev-suspend`: every Deployment and the
er-postgres StatefulSet to 0 replicas, Deployments first. Runner Jobs are
never touched — the predicate guarantees none is active when this fires.

Deliberately dependency-light: psycopg (already in the er-api image this pod
runs) for the two queries, and the Kubernetes API spoken over stdlib urllib
with the ServiceAccount token — the image carries no kubectl and does not
need one for two GETs and a handful of scale-subresource PATCHes.
"""

import json
import os
import ssl
import sys
import urllib.request

import psycopg

SERVICEACCOUNT = "/var/run/secrets/kubernetes.io/serviceaccount"

# The non-terminal job states: QUEUED plus ACTIVE_STATES, mirrored from
# server/src/erserver/policy.py (the states in which a job is queued, holds
# its org's one-active slot, or is winding down).
ACTIVE_JOB_STATES = ["queued", "dispatching", "running", "canceling"]


def kube(method: str, path: str, body: dict | None = None) -> dict:
    """One Kubernetes API call, authenticated as the pod's ServiceAccount."""
    with open(f"{SERVICEACCOUNT}/token", encoding="ascii") as handle:
        token = handle.read().strip()
    url = (
        f"https://{os.environ['KUBERNETES_SERVICE_HOST']}"
        f":{os.environ['KUBERNETES_SERVICE_PORT']}{path}"
    )
    request = urllib.request.Request(url, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("Accept", "application/json")
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        request.add_header("Content-Type", "application/merge-patch+json")
    context = ssl.create_default_context(cafile=f"{SERVICEACCOUNT}/ca.crt")
    with urllib.request.urlopen(request, data=data, context=context, timeout=10) as response:
        return json.load(response)


def fetch_one(connection: psycopg.Connection, sql: str, params: tuple, default: tuple) -> tuple:
    """One row, with a missing table mapped to the caller's default."""
    try:
        with connection.cursor() as cursor:
            cursor.execute(sql, params)
            return cursor.fetchone()
    except psycopg.errors.UndefinedTable:
        connection.rollback()  # reset the aborted transaction for the next query
        return default


def main() -> int:
    with open(f"{SERVICEACCOUNT}/namespace", encoding="ascii") as handle:
        namespace = handle.read().strip()
    threshold_hours = float(os.environ.get("ER_IDLE_THRESHOLD_HOURS", "4"))

    # What a suspend would scale, and where each stands now. Deployments
    # first, then the StatefulSet — the same order as `make dev-suspend`.
    deployments = kube("GET", f"/apis/apps/v1/namespaces/{namespace}/deployments")["items"]
    postgres = kube("GET", f"/apis/apps/v1/namespaces/{namespace}/statefulsets/er-postgres")
    targets = [
        ("deployments", item["metadata"]["name"], item["spec"].get("replicas", 0))
        for item in deployments
    ]
    targets.append(("statefulsets", "er-postgres", postgres["spec"].get("replicas", 0)))

    # Self-exclusion: this pod still fires on schedule in a suspended
    # namespace (CronJobs schedule regardless of scaled-down Deployments).
    # Resume is manual by design (§5.2) — exit quietly, resurrect nothing.
    if all(replicas == 0 for _, _, replicas in targets):
        print("already suspended; nothing to do")
        return 0
    if postgres["spec"].get("replicas", 0) == 0:
        # Deployments up with Postgres at 0 is a half-finished manual suspend;
        # without the database the predicate cannot be evaluated. Leave it.
        print("er-postgres is at 0 but a deployment is up; leaving the half-state to a human")
        return 0

    dsn = os.environ.get("ERSERVER_DSN", "")
    if not dsn:
        # dev-up installs er-dev-namespace minutes before er-platform projects
        # er-erserver-env (§9); until then there is nothing worth suspending.
        print("no ERSERVER_DSN projected yet (er-platform not installed); skipping")
        return 0

    with psycopg.connect(dsn, connect_timeout=10) as connection:
        # Compared in SQL against the database clock — no pod/DB clock skew.
        # An empty table yields (NULL, NULL), i.e. not idle: §5.2's predicate
        # is about an abandoned environment, not one nobody has logged into
        # yet. A missing table means the erweb migrations have not run — same
        # answer.
        idle, last_seen = fetch_one(
            connection,
            "SELECT max(last_seen_at) < now() - %s * interval '1 hour', max(last_seen_at) "
            "FROM erweb.sessions",
            (threshold_hours,),
            default=(None, None),
        )
        (active_jobs,) = fetch_one(
            connection,
            "SELECT count(*) FROM jobs WHERE state = ANY(%s)",
            (ACTIVE_JOB_STATES,),
            default=(0,),  # table missing: ensure_schema has not run, so no jobs
        )

    if idle is not True:
        print(f"not idle: max(last_seen_at)={last_seen} within {threshold_hours}h (or no sessions)")
        return 0
    if active_jobs:
        print(
            f"idle (last seen {last_seen}) but {active_jobs} job(s) in "
            f"{ACTIVE_JOB_STATES}; not suspending — §5.2's second clause"
        )
        return 0

    print(f"idle: max(last_seen_at)={last_seen} > {threshold_hours}h ago, no active jobs")
    for kind, name, replicas in targets:
        if replicas == 0:
            continue
        kube(
            "PATCH",
            f"/apis/apps/v1/namespaces/{namespace}/{kind}/{name}/scale",
            {"spec": {"replicas": 0}},
        )
        print(f"scaled {kind}/{name} {replicas} -> 0")
    print(f"namespace {namespace} suspended; `make dev-resume DEV={namespace}` brings it back")
    return 0


if __name__ == "__main__":
    sys.exit(main())
