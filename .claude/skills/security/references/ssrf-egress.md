# SSRF and egress

The repo makes real outbound requests from the server, and the cluster's egress is unrestricted by
design — so an unfiltered URL is a direct line to cloud metadata and internal services.

## Contents
- Webhook SSRF (the headline)
- The LLM tool loop (data egress + prompt injection)
- The honest limit: what a local test can and cannot prove
- The URL guard to build
- How to prove / how to lock

## Webhook SSRF (the headline) — FIXED at the application layer

**Status: the application-layer guard is built** (`er-server/src/urlguard.rs`). What follows is kept
because the reasoning still governs any NEW outbound path, and because the network leg is still open.

Before the fix, `POST /v1/orgs/{org}/webhooks` (`er-server/src/lib.rs` `create_webhook`, `Role::Admin`)
validated the URL only as `!url.trim().is_empty()`, and the dispatcher then POSTed job payloads to it
(`er-server/src/webhook.rs` `post`/`emit`, plain `reqwest`, no scheme/host/IP filter). Infra gives no
containment: `infra/k8s/charts/er-dev-namespace/templates/networkpolicy.yaml` says "Egress is
deliberately unrestricted."

High-value targets an attacker webhook URL could reach from inside the cluster:
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

## The URL guard, as built

`er-server/src/urlguard.rs` — `check(url, Egress) -> Result<(), UrlRejection>`, called by
`create_webhook` (so a bad URL is a 422 and never reaches the table) **and** by the delivery loop
before every POST (so DNS rebinding does not get a second chance). What it enforces:

- scheme must be `https` (reject `http`, `file`, `gopher`, etc.)
- **resolve the host, then check every resolved IP** (not the hostname) against deny ranges:
  loopback `127.0.0.0/8` + `::1`, link-local `169.254.0.0/16` + `fe80::/10`, private
  `10/8`, `172.16/12`, `192.168/16`, unique-local `fc00::/7`, and `0.0.0.0`/`::`
- reject credentials in the URL, non-default dangerous ports if you scope them
- cap/forbid redirects (or re-run the guard on each hop)

Resolve-then-check is what defeats DNS rebinding; a name-only check is bypassable.

Two traps found while building it, both now locked by enumerated unit tests:

- **`::1` was allowed.** `Ipv6Addr::to_ipv4` also maps the deprecated IPv4-*compatible* form
  `::a.b.c.d`, and `::1` is in that form — so unwrapping IPv4-in-IPv6 *before* judging turned THE IPv6
  loopback into `0.0.0.1`, which is not loopback, not private and not unspecified. Judge `::1` / `::`
  as IPv6 first, unwrap second.
- **`https://[::1]/` was allowed.** `Url::host_str` keeps the brackets on an IPv6 literal, which is not
  a resolvable name, so `lookup_host` failed — and "a lookup failure is not a rejection" (right for a
  domain that is not stood up yet) then waved it through. IP literals are parsed and judged directly,
  before any resolution.

**There is no off switch.** No env var, no config field: a guard configuration can disable is a guard
that gets found disabled. The one unguarded path is `webhook::emit_with_egress(.., Egress::Unguarded)`,
which exists so the delivery test can POST to `127.0.0.1`, and `er-server/tests/egress_guard.rs` fails
if any file under `er-server/src` names it.

## How to prove / how to lock

| Claim | PoC | Test |
|---|---|---|
| Webhook accepts an SSRF URL | ~~register `http://169.254.169.254/...` → 2xx~~ **locked** | `urlguard.rs` unit tests (every denied range, enumerated) + `pg_webhooks::registration_refuses_an_url_the_server_may_not_reach` → 422 |
| Delivery hits a blocked host | ~~queue a job; delivery attempts loopback~~ **locked** | `pg_webhook_deliver::the_guarded_path_refuses_a_delivery_to_a_blocked_address` — refused, recorded, and the job still succeeds |
| The guard has no bypass | n/a | `er-server/tests/egress_guard.rs` scans `er-server/src` for the test-only seam |
| IMDS reachable from a pod | (live, EKS only — out of scope for the fence) | manual §17 probe, *not* a unit test |
| Assistant tool org is caller-bound | injected "fetch org X" prompt still targets the session org | `frontend/tests/unit/assistant-tools.test.ts` + `frontend/tests/functional/assistant.spec.ts` |
