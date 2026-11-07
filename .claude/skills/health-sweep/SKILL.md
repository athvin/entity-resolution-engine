---
name: health-sweep
description: Runs a read-only health sweep over a deployed environment and turns what it finds into board cards — reads the CloudWatch alarms and whether their SNS subscriptions are confirmed, the `/er/<env>/` log groups for error and panic lines, OpenObserve for error-status traces, pod, event and restart state on the EKS cluster, and the control plane's failed or stuck jobs, then clusters symptoms by root cause, dedupes against open issues, and files new issues with evidence as pointers, never pasted log payload. Use when asked to check dev or prod health, hunt errors in the running system, triage cloud logs, metrics or alarms into tickets, or when an alarm fired and the question is what broke. Invoke as /health-sweep [env]. For emitting telemetry and what a span means see `observability`; for CLI profiles, quotas and the bill see `aws-account-operations`; for board mechanics see `project-board`; for picking up a filed card see `claim-ticket`.
argument-hint: [dev|prod]
disable-model-invocation: true
---

# Health sweep

Arguments: `$ARGUMENTS` — the environment to sweep; empty means `dev`. The
default window is the last 24 hours, or back to the newest card a previous
sweep filed, whichever is shorter.

The sweep is **read-only against AWS and Kubernetes**. Its only writes are
GitHub issues, filed at the end, after dedup. A finding is never fixed
mid-sweep — not a pod restart, not a config edit, not an `aws` mutation: the
product of a sweep is cards someone claims (`claim-ticket`) and fixes under
review. If something looks actively on fire, say so at the top of the report
and stop — escalation is the user's call, not the sweep's.

Scope boundary: this sweep reads the **running system**. A red CI run, a
stale green or a stuck merge is `ci-and-supply-chain`'s subject, not a health
finding.

## Two rules that are not judgment calls

**Evidence is pointers, never payload.** This repository is public, so an
issue body is public. The logs of a deduplication engine carry names, emails
and addresses on their bad days — a database error message routinely embeds
the offending record value, which is exactly why events are never exported to
the span store (`observability` owns that boundary). A card therefore carries
the log group, the exact query, the time window, and trace/run/job ids — and a
*described* error, never a pasted log line. If a quoted fragment is genuinely
necessary, redact every value field by hand and say in the issue that you did.

**One card per root cause, deduped against what is already filed.** Fifty
identical stack traces are one finding. A sweep must also be idempotent:
search open issues before filing, and when an open issue already covers the
root cause, add one corroborating comment (new window, new count, pointers)
instead of a duplicate card — and no comment at all if the newest comment
already covers the same occurrence.

## Phase 0 — preconditions

Check these first, because each fails late and ambiguously once the sweep is
moving:

- `aws sts get-caller-identity --profile er-readonly` — an expired SSO token
  is the usual reason reads stop answering; the refresh is interactive, so ask
  the user to run `! aws sso login --profile er-readonly` and wait.
- `kubectl --context er-<env> get ns` — backs the cluster phases.
- `gh auth status` — filing needs repo scope; reading the board needs
  `read:project` (`project-board` has the refresh incantation).

Missing cluster access degrades the sweep rather than aborting it: alarms,
logs and spend are all readable with the CLI alone. The report must say which
sources were swept and which were skipped — a sweep that silently covered half
the surface reads as a clean bill.

## Phase 1 — the sweep, in evidence order

Copyable commands for every step:
[references/sweep-recipes.md](references/sweep-recipes.md).

1. **Alarms, then their reachability.** Anything in `ALARM` state, and any SNS
   subscription still pending confirmation — an alarm that reaches nobody is
   itself a finding, and an unconfirmed subscription looks identical to a
   working one in the console. Coverage is deliberately thin (runaway
   detectors, not budgets — the reasoning is in the alarm Terraform), so
   "nothing alarmed" is never "nothing wrong".
2. **Cluster state.** Pods not Running/Succeeded, restart counts, recent
   Warning events, across namespaces. Two discriminations route elsewhere: a
   Pending pod is the vCPU-quota-versus-ResourceQuota table in
   `aws-account-operations`, and a crashlooping *replacement* pod behind a
   green deploy means a rejected config while the old pod serves — read
   restart counts, never deploy conclusions.
3. **The control plane's own view.** Port-forward the API and read failed and
   stuck jobs. The routes are in the OpenAPI document (`api-contract`); the
   queue semantics are `er-server-control-plane`'s subject.
4. **Logs.** CloudWatch Logs Insights over the `/er/<env>/` groups for
   error/panic lines in the window — the containers group carries every
   `eprintln!` and every pipeline stage. A result set exactly at the query's
   `limit` is a floor, not a count.
5. **Traces.** OpenObserve, for error-status spans in the window. Port-forward
   the store, never the collector — the collector binds its receiver to the
   pod IP, so a localhost port-forward gets connection-refused and reads as an
   outage. What a span carries and what it deliberately does not is the
   `observability` skill's subject.
6. **Spend signals.** Budgets say *that* the bill moved; the CUR answers *why*
   (`aws-account-operations`). The log-ingest alarms from step 1 already watch
   for telemetry runaway.

## Phase 2 — from symptom to finding

Cluster everything by root cause before writing anything. Known-benign shapes
that must not become cards:

| Shape | Why it is not a finding |
|---|---|
| a task pod exiting `10` | success — "nothing to do"; reading it as failure reports healthy no-op runs as broken (`er-engine-ops`) |
| schema-reconciliation spans with no parent | orphan roots by design, emitted before a run span exists (`observability`) |
| an `[aw] … failed` issue on the board | the gh-aw workflows are disabled and fail closed; housekeeping, not health |
| an empty job queue | idle is healthy; absence of runs is a finding only if runs were scheduled |

For what remains, a finding is: the root cause as best understood, the blast
radius (which tenant, environment, stage or surface), the evidence pointers,
and what *would* have alerted — often nothing, in which case the coverage gap
is a second, separate finding, usually labelled `obs-backlog`.

## Phase 3 — dedupe, then file

Dedupe first (recipes). Then one issue per root cause via `gh issue create` —
the board's auto-add puts it on the board and sets `Status` itself, so no
`item-add` and no Status edit (`project-board` owns the automations).

The card: a title stating the failure, not the symptom; a body with
environment, window, evidence pointers, blast radius, suspected cause, and a
"how to reproduce the read" block holding the exact queries. Label with the
class from `project-board`'s table — `followup` for operational findings,
`obs-backlog` for telemetry-pipeline gaps, none when none fits. Leave
`Priority` and `Estimate` unset: absence is the backlog, and urgency belongs
in the final report where a human decides. Never set `Done` on anything — the
automation closes the underlying issue.

## Final report

```
## health-sweep: <env>, <window>
**Swept:** alarms, cluster, control plane, logs, traces | skipped: <which, why>
**Filed:** #<n> <title> …
**Corroborated:** #<n> (comment added) …
**Benign:** <shape — why>
**Urgent:** <anything a human should look at today, or "nothing">
```

A sweep that files nothing is a good sweep; say so plainly rather than
manufacturing a finding.
