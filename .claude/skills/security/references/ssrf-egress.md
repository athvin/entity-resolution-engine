# SSRF and egress

The repo makes real outbound requests from the server, and the cluster's egress is unrestricted by
design — so an unfiltered URL is a direct line to cloud metadata and internal services.

## Contents
- Webhook SSRF (the headline)
- The LLM tool loop (data egress + prompt injection)
- The honest limit: what a local test can and cannot prove
- The URL guard to build
- How to prove / how to lock

## Webhook SSRF (the headline)

`POST /v1/orgs/{org}/webhooks` (`er-server/src/lib.rs` `create_webhook`, `Role::Admin`) validates the
URL only as `!url.trim().is_empty()`. The dispatcher then POSTs job payloads to it
(`er-server/src/webhook.rs` `post`/`emit`, plain `reqwest`, no scheme/host/IP filter, no redirect
limit). Infra gives no containment: `infra/k8s/charts/er-dev-namespace/templates/networkpolicy.yaml`
says "Egress is deliberately unrestricted."

High-value targets an attacker webhook URL can reach from inside the cluster:
- `http://169.254.169.254/latest/meta-data/...` — cloud instance metadata (IMDS). On EKS this can
  surface role credentials.
- cluster-internal services (`http://<svc>.<ns>.svc.cluster.local`), `http://localhost:<port>`.
- redirect-based bypass (register an allowed host that 302s to a blocked one).
- DNS rebinding (a hostname that resolves to a public IP at check time, a private IP at fetch time).

## The LLM tool loop (data egress + prompt injection)

`frontend/src/app/api/orgs/[org]/assistant/route.ts` + `frontend/src/lib/assistant/tools.ts` run up to
8 tool-call rounds with 8 read tools, using the *viewer* credential. Two concerns:
- **Data egress to a subprocessor** — tenant record data (names, emails, metadata) is sent to the
  Anthropic API. This is noted in `docs/infrastructure.md` §8.6 as needing a DPA + disclosure. It is a
  governance finding, not a code bug, but name it.
- **Prompt injection** — tenant-controlled record text can try to steer later tool calls. The
  defences to lock: the toolbox is a *closed read-only allowlist*; every tool forwards to
  `/v1/orgs/${access.org}/...` where `org` is bound to the session and never caller/model-supplied; no
  tool output is inlined into a position that becomes a new instruction. Same shape in
  `enrich/route.ts`.

## The honest limit: what a local test can and cannot prove

**You cannot reproduce the k8s-egress-to-IMDS leg locally** — there is no metadata service answering
on a dev box, and the NetworkPolicy isn't exercised. So split the finding, and say so in the report:

- **Application-layer URL guard** — fully testable, and IS the lockable PoC. A unit test proving the
  guard rejects loopback/private/link-local/non-https URLs is the regression lock.
- **Network egress (NetworkPolicy)** — defence-in-depth. Its only proof is the manual
  `docs/infrastructure.md` §17 behavioral probe ("IMDS unreachable from a pod"). Do not claim the unit
  test covers this leg.

Never fire a real request at `169.254.169.254` except in `/pentest live` against a confirmed-local
target — see [../../pentest/references/modes.md](../../pentest/references/modes.md).

## The URL guard to build

The fix is a single function the webhook registration and the dispatcher both call before any request:

- scheme must be `https` (reject `http`, `file`, `gopher`, etc.)
- **resolve the host, then check every resolved IP** (not the hostname) against deny ranges:
  loopback `127.0.0.0/8` + `::1`, link-local `169.254.0.0/16` + `fe80::/10`, private
  `10/8`, `172.16/12`, `192.168/16`, unique-local `fc00::/7`, and `0.0.0.0`/`::`
- reject credentials in the URL, non-default dangerous ports if you scope them
- cap/forbid redirects (or re-run the guard on each hop)

Resolve-then-check is what defeats DNS rebinding; a name-only check is bypassable.

## How to prove / how to lock

| Claim | PoC | Test |
|---|---|---|
| Webhook accepts an SSRF URL | register `http://169.254.169.254/...` → currently 2xx | **(a)** unit in `webhook.rs` for the guard; **(b)** extend `er-server/tests/pg_webhooks.rs` → register blocked URL = 422 |
| Delivery hits a blocked host | queue a job; delivery attempts loopback | extend `er-server/tests/pg_webhook_deliver.rs` → delivery refused |
| IMDS reachable from a pod | (live, EKS only — out of scope for the fence) | manual §17 probe, *not* a unit test |
| Assistant tool org is caller-bound | injected "fetch org X" prompt still targets the session org | `frontend/tests/unit/assistant-tools.test.ts` + `frontend/tests/functional/assistant.spec.ts` |
