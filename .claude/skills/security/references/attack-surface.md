# Attack-surface inventory

The scoped map `/pentest` loads at recon, and the inventory a security review consults. Line numbers
are hints that drift — anchor on the **symbol name**. Refresh this file when `build_routes` or the BFF
route tree changes.

## Contents
- Topology and trust boundaries
- er-server: the sinks that matter
- BFF: the internet-facing routes
- The sink catalog (where caller input reaches something dangerous)
- Known gaps, ranked

## Topology and trust boundaries

```
internet ──▶ BFF (Next.js, frontend/src/)  ──erk_ key / operator token──▶  er-server (rust/crates/er-server/)  ──▶ Postgres (control plane)
             cookie session er_session                                      Principal extractor + guard()        ──▶ DuckLake per-org (data plane)
             holds 3 per-org service keys                                                                        ──▶ outbound reqwest (webhooks)
             + global operator token
```

- The browser never holds an `erk_` key. The BFF is the confused-deputy risk: it calls `er-server`
  with privileged keys on behalf of a cookie session.
- `er-server` trusts its caller's bearer token fully; its defence is `guard()` per handler, not
  network position. Treat every handler as directly reachable by anyone with *a* valid key.
- No legacy Python remains; doc comments that mention FastAPI / `*.py` paths are stale.

## er-server: the sinks that matter

- **Entrypoint** `main.rs` — binds `ER_SERVER_ADDR` (default `0.0.0.0:8080`), runs `ensure_schema`,
  optionally spawns the in-process dispatcher (`ER_DISPATCH_IN_PROCESS`). Boot order has a known
  deadlock (start API before dispatcher) — relevant to `/pentest live` bring-up.
- **Routes** `build_routes` in `lib.rs` — ~55 routes, nearly all `/v1/orgs/{org}/...`.
- **Auth** `Principal` extractor (`FromRequestParts`); operator token compared with `ct_eq`
  (constant-time); `erk_<id>_<secret>` → `pg::lookup_key` → `ct_eq(stored, sha256(secret))`.
- **Authorization** `guard(p, org, minimum)` and `operator_only(p)`. Every handler was confirmed to
  guard (some via delegation — `record_overlay_action`, `transition_org`). No unguarded handler was
  found; a posterity gate should keep it that way.
- **Outbound email** `POST /v1/email` → `mail.rs`. `operator_only`, and the recipient is entirely
  caller-controlled, so anything holding the operator token can make this service send
  DupeZero-branded mail to any address — treat the operator token's blast radius as including that.
  Rendering happens before the backend is consulted, so an unknown template or missing parameter is a
  422 rather than a blank delivery. Two things to weigh in any review that touches a *caller* of the
  relay: whether an unauthenticated flow can reach it (self-serve signup will — that is what its rate
  limiting is for), and that `Backend::Log` writes the rendered body, which carries a single-use
  credential, to the pod log. `Log` is dev-only for exactly that reason.
- **No middleware**: no CORS, no rate limiting, no request timeout, no concurrency limit, no explicit
  body-size layer beyond axum's 2 MB `Json` default. `/healthz` is the only anonymous route.
- **Connect-per-request Postgres** (`connect_pg`) — no pool. A connection-exhaustion lever.
- **Pre-routing URI rewrite** `rewrite_action_path` (`lib.rs`, in an outer `fallback_service`) maps
  `.../x:verb` → `.../x/verb` for 8 `ACTION_VERBS`. A route-confusion primitive — fuzz ids with `:`,
  `%3A`, double-colon, and params ending in a verb.

## BFF: the internet-facing routes

- **Session** `frontend/src/lib/auth/session.ts` — 256-bit opaque token, sha256 at rest, 30d sliding,
  `httpOnly` + `sameSite: "lax"` + `secure` (unless `ERWEB_INSECURE_COOKIES=1`).
- **Passwords** `frontend/src/lib/auth/password.ts` — argon2id at OWASP params.
- **Edge middleware** `proxy.ts` — cookie-*presence* check only; real verification is per-route.
  Public paths: `/login`, `/invite`, `/reset`, `/api/auth/password-reset`, `/api/invites`.
- **Authorization** `requireOrgAccess` (`frontend/src/lib/bff/proxy.ts`) — non-members 404.
  **Super admins get implicit `role: "admin"` on every org** (`viaOperator: true`, uses the operator
  token). Impersonation (`app/api/impersonation/route.ts`) has a 60-min TTL and replaces super-admin
  powers while active.
- **Credential vault** `frontend/src/lib/db/crypto.ts` — AES-256-GCM with `org\0role` as AAD (a
  stolen row can't be replayed onto another org). Well done — a *positive* invariant to lock.
- **Upstream URL building** — several routes interpolate `org`/ids into `new URL(\`/v1/orgs/${org}\`)`
  (`admin.ts`, `impersonation/route.ts`, `imports/route.ts`, `register/route.ts`). Provisioning
  validates `^[a-z0-9][a-z0-9_-]*$`, but `register` takes the path segment **unvalidated**.
- **LLM tool loop** `app/api/orgs/[org]/assistant/route.ts` + `lib/assistant/tools.ts` — up to 8
  tool-call rounds, 8 read tools run with the *viewer* credential; tenant record data (names, emails)
  is fed to the model (Anthropic API) and can steer later tool calls. Same shape in `enrich/route.ts`.
- **Dangling route**: BFF `POST /api/orgs/[org]/imports` (multipart, 256 MiB) forwards to a
  `POST /v1/orgs/{org}/imports` that **does not exist** upstream (router has `get(imports_list)` only).

## The sink catalog (where caller input reaches something dangerous)

| Sink | Location | Caller input | Guard today |
|---|---|---|---|
| **Outbound HTTP** | `webhook.rs` `post`/`emit`; dispatcher | webhook URL (`lib.rs` `create_webhook`, Admin) | `!url.trim().is_empty()` only — **SSRF** |
| **Raw SQL (config)** | `er-config/src/lib.rs` `BlockingRule.expr`, `Training.*_rules` → `dedupe-core/blocking*.rs`, `linker.rs` | config body (Admin publish; `training` operator-only) | partial: `config_check.rs` column refs for `blocking[].expr` only |
| **`format!` SQL** | `er-golden/src/{records,survivorship,master}.rs`, `er-standardize/src/lib.rs` | config attribute/source names | uneven: `CANONICAL_ATTRIBUTES` allowlist, `quote_literal`, `single_quote`, `alias_key` sanitizer — **audit each path has one** |
| **Filesystem path** | `dispatcher.rs` `{drop_root}/{source}.csv` | config `sources` keys | **none** — path traversal |
| **CSV export cell** | `read.rs` `merge_plans_csv`/`csv_cell` | tenant record names/emails | escapes `" , \n` — **no leading `= + - @ \t \r`** (formula injection) |
| **IN-list expansion** | `read.rs` `keys_from_query` | `key=` query values | caps 50 keys but doesn't strip `\u{1f}` delimiter — cap bypass |
| **LLM prompt** | `lib/assistant/tools.ts` | tenant record data | tool allowlist, viewer cred — prompt-injection steering |
| **Record graph** | `lib.rs` `add_assertion` (Steward) | arbitrary `a`/`b` keys | **no existence check** — forced merge/cut |

## Known gaps, ranked

**High**
1. Webhook SSRF → cloud metadata (`169.254.169.254`); k8s egress unrestricted (`networkpolicy.yaml`:
   "Egress is deliberately unrestricted").
2. Tenant-isolation invariant: no two `orgs.env` rows may share `ER_LAKE_CATALOG`/`ER_LAKE_DATA_PATH`.
3. Config-as-code SQL: `blocking[].expr`, `training.deterministic_rules`, `em_blocking_rules`.
4. Assertion abuse: forced merges via nonexistent/foreign keys, no volume cap, no 4-eyes.

**Medium**
5. Prompt injection / PII-to-LLM (assistant + enrich).
6. CSV formula injection (`merge_plans_csv`; also BFF segments/export).
7. Login brute-force (no rate limit/lockout; argon2 is the only brake).
8. `:action` path-rewriter confusion.
9. Request-lane DoS: synchronous `match-preview` full pipeline; connect-per-request Postgres.
10. Config-publish panics (`er-standardize` index panic on missing canonical column); no threshold ranges.

**Low**
11. `MAX_KEYS` `\u{1f}` delimiter cap bypass.
12. Path traversal via config source names.
13. Unvalidated `org` path segment in BFF `admin/orgs/[org]/register/route.ts`.
14. Unsalted SHA-256 API-key hashes (deliberate; high-entropy secrets; non-standard).
15. Dangling import-upload route (no upstream handler).

**Positive invariants worth locking (defence that exists — keep it)**
- Credential vault AAD binding (`org\0role`).
- Every DTO `#[serde(deny_unknown_fields)]`.
- `guard()`/`operator_only` on every route.
- No `lake.main.<x>` / `query_strings` in `er-server/src` (existing posterity gate).
