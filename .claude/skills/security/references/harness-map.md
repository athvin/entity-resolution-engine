# Harness map: category → exact test → how to extend

The file that keeps "prove it + lock it" honest. For each vulnerability class it names the **real**
existing test file whose pattern you clone, the test *kind*, and the move. This is the load-bearing
reference — if a finding has no row here, add the row before writing the test.

## Contents
- The six test shapes in this repo
- The map (one row per class)
- How to clone each shape
- What can't be a hermetic test (and what to do instead)

## The six test shapes in this repo

| Shape | Run by | Pattern exemplar | Use for |
|---|---|---|---|
| **Static posterity gate** | `cargo test` | `rust/crates/er-schema/tests/freeform_sql.rs`, `declared_names.rs` | structural invariants across the whole workspace |
| **Env-gated pg integration** | `cargo test` w/ `ER_TEST_PG_DSN` | `rust/crates/er-server/tests/pg_auth.rs`, `pg_orgs.rs`, `pg_steward.rs`, `pg_webhooks.rs` | handler behaviour against real Postgres |
| **Plan-vector** | `cargo test -p er-sql-check` | `rust/crates/er-sql-check/src/stage.rs` tests | generator SQL over real config vectors |
| **Co-located unit** | `cargo test` | `#[cfg(test)] mod tests` in `webhook.rs`, `config_check.rs`, `dispatcher.rs` | pure functions (URL guard, escaping, path building) |
| **Playwright functional** | `pnpm test:e2e` | `frontend/tests/functional/{security,auth,impersonation}.spec.ts` | BFF behaviour against the mock stack |
| **Vitest unit (frontend)** | `pnpm test` | `frontend/tests/unit/*.test.ts` (note: `.test.ts`, not `.spec.ts`) | pure TS (limiter, tool allowlist) |

## The map

| # | Class | Code under test | Test home | Shape |
|---|---|---|---|---|
| 1 | Lake-catalog uniqueness (tenant isolation) | `er-server/src/read.rs` `open_org_lake` | **NEW** `er-server/tests/pg_tenant_isolation.rs` (clone `pg_orgs.rs`) | env-gated pg |
| 2 | Cross-org read → 404 | `er-server/src/lib.rs` `guard` | extend `er-server/tests/pg_auth.rs` | env-gated pg |
| 3 | Webhook SSRF → IMDS | `er-server/src/webhook.rs` `post`/`emit`; reg in `lib.rs` `create_webhook` | **(a)** unit in `webhook.rs` for a new URL guard; **(b)** extend `tests/pg_webhooks.rs` (register blocked → 422) + `tests/pg_webhook_deliver.rs` (deliver blocked → refused) | unit + env-gated pg |
| 4 | Config-as-code raw SQL | `er-config/src/lib.rs` `BlockingRule.expr`, `Training.deterministic_rules`, `em_blocking_rules`; `er-server/src/config_check.rs` | extend `config_check.rs` `#[cfg(test)] mod tests` (parse the two `training` lists it ignores today) | unit |
| 5 | CSV formula injection | `er-server/src/read.rs` `merge_plans_csv`/`csv_cell` | co-located unit in `read.rs` | unit |
| 6 | MAX_KEYS `\u{1f}` bypass | `er-server/src/read.rs` `keys_from_query` | co-located unit in `read.rs` | unit |
| 7 | `:action` path-rewriter confusion | `er-server/src/lib.rs` `rewrite_action_path` | co-located unit, or `er-server/tests/http.rs` (no pg needed) | unit |
| 8 | Path traversal via config source names | `er-server/src/dispatcher.rs` (`{drop_root}/{source}.csv`) | unit in `dispatcher.rs` + a reject case in `config_check.rs` | unit |
| 9 | Assertion abuse (forced merge) | `er-server/src/lib.rs` `add_assertion` | extend `er-server/tests/pg_steward.rs` (nonexistent key → 422, not staged) | env-gated pg |
| 10 | Config-publish panic / threshold range | `er-standardize/src/lib.rs` (`spec.columns[...]` index); thresholds in `er-config` | `config_check.rs` unit + a `stage.rs` plan-vector case (missing mapped column → error, not panic) | unit + plan-vector |
| 11 | Request-lane DoS | `er-server/src/lib.rs` `match_preview`, `connect_pg` | **NEW** static gate: handlers use a pooled client; preview work is bounded — *partial coverage, flag it* | static gate |
| 12 | Mass-assignment hardening | `er-config` + `er-server` request/config DTOs | **NEW** static gate: every request/config DTO has `#[serde(deny_unknown_fields)]` | static gate |
| 13 | Unguarded / dangling routes | `er-server/src/lib.rs` `build_routes` | **NEW** static gate: every declared route is reachable + reaches `guard`/`operator_only` | static gate |
| 14 | Prompt injection / PII→LLM | `frontend/src/lib/assistant/tools.ts` | **NEW** `frontend/tests/unit/assistant-tools.test.ts` (closed allowlist, org bound to session) + **NEW** `frontend/tests/functional/assistant.spec.ts` (injected prompt still 404s other org) | vitest + Playwright |
| 15 | Login brute-force | `frontend/src/app/api/auth/login/route.ts` | limiter vitest unit + extend `frontend/tests/functional/auth.spec.ts` (N fails → 429/lockout) | vitest + Playwright |
| 16 | Unvalidated `org` segment | `frontend/src/app/api/admin/orgs/[org]/register/route.ts` | vitest unit on the validator; functional case | vitest + Playwright |
| 17 | Error-message info leak | `er-server/src/lib.rs` `detail_err` callers (`format!("db: {e}")`) | **NEW** static gate: no `format!("...{e}")`-shaped error reaches a response body | static gate |
| 18 | Secrets / IaC | `infra/terraform/.../builder.tf`, `modules/eks/main.tf`, `frontend/dev/`, `.claude/settings.local.json` | **deferred to the CI layer** — documented in `secrets-iac.md`, not a Rust/TS test | (CI, later) |

## How to clone each shape

**Static posterity gate** (clone `freeform_sql.rs`). A plain `#[test]` that walks the workspace
(`walkdir` over `rust/crates/*/src`), collects offenders into a `Vec<String>`, and asserts it is
empty with a message that *names each offender and the remedy*. Always include a sanity assert
(`assert!(files_scanned > N, "scan found too few files — the walk is broken")`) so a broken walk can't
pass vacuously. The message is the UX — copy the "here is what to do" tone of `freeform_sql.rs`.

**Env-gated pg integration** (clone `pg_auth.rs` / `pg_orgs.rs`). `#[tokio::test]`; first line:
`let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else { eprintln!("skipping: ER_TEST_PG_DSN unset"); return; };`
Build `app(AppState::with_pg(&dsn).await?)`, drive requests with `oneshot(req(...))`, assert
status + body. Seed orgs/keys via the same helpers the neighbouring tests use. These run under
`make check-e2e` and the CI `ducklake-e2e` job; they self-skip locally without the DSN.

**Plan-vector** (clone `stage.rs` tests). Feed the generator a real `er_config` fixture (`TEST_CONFIG`
there), plan every shape via DataFusion, assert it plans (or, for the panic case, that a malformed
config returns `Err`, not a panic). Add new vectors beside the existing person/account ones.

**Co-located unit** (clone the `#[cfg(test)] mod tests` already in `webhook.rs`/`config_check.rs`).
Pure function in, assertion out. For the URL guard: a table of `(url, expected_allowed)` covering
`https` public (allow), `http` (deny), loopback, `10./172.16/192.168`, `169.254.x` link-local,
`[::1]`, and a DNS name that resolves to a private IP (resolve-then-check, to catch rebind).

**Playwright functional** (clone `security.spec.ts`). It already asserts no `erk_`/`mock-operator-token`
appears in any response — extend that file or add a sibling using the same page/fixture setup.

**Vitest unit** (clone an existing `frontend/tests/unit/*.test.ts`, e.g. `crypto.test.ts`). `.test.ts`
suffix, picked up by `pnpm check`/`pnpm test`.

## What can't be a hermetic test (and what to do instead)

- **k8s-egress SSRF to IMDS** — not locally reproducible. The app-layer URL guard unit test IS the
  lockable PoC; the egress NetworkPolicy is defence-in-depth whose proof is the manual
  `docs/infrastructure.md` §17 behavioral probe. State this in the finding; don't claim the unit test
  covers the network leg. See [ssrf-egress.md](ssrf-egress.md).
- **Webhook *delivery*, brute-force, request-lane DoS, `match-preview`** — need a running stack.
  Express the PoC AS an env-gated integration / Playwright test on the existing tiers
  (`ER_TEST_PG_DSN`, `DEDUPE_E2E_DUCKLAKE`, `FRONTEND_E2E`) against `frontend/dev/compose.yaml`. PoC
  and regression test are then the same artifact. `/pentest live` runs these; hermetic mode skips them
  and says so.
- **Request-lane DoS** has no crisp functional assertion — settle for the structural gate (#11) and
  mark it *partial coverage* in the report.
