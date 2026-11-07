# Not a finding

Most of this skill is this file. That ratio is deliberate and it is the one
thing the open-source review prompts agree on: a reviewer's quality is set by
what it declines to say, because the cost of a wrong finding is not the wrong
finding — it is the author skimming the next report.

Read this **before** fanning out, and apply it again after verification.

## Contents

- [Already a red test](#already-a-red-test)
- [The standing exclusions](#the-standing-exclusions)
- [Security precedents](#security-precedents)
- [House precedents — correct things that read as defects](#house-precedents--correct-things-that-read-as-defects)
- [Things not to ask the author for](#things-not-to-ask-the-author-for)

## Already a red test

This repo answers "this cannot come back" with a source scan in a test file
rather than with a convention, so a surprising amount of what looks like a
review finding is a build failure the author has not reached yet.

<!-- BEGIN GENERATED: review-gates -->
| Gate | What it forbids | Runs in |
|---|---|---|
| `er-entity-gen/tests/workspace_self_contained.rs` | The posterity gate for the workspace boundary: no source under `rust/` may `include_str!` / `include_bytes!` / `include!` a path that escapes the cargo workspace root. | `make check` |
| `er-errors/tests/display_no_source.rs` | The posterity gate for error `Display`: no `#[error("…")]` message may interpolate its own source. | `make check` |
| `er-schema/tests/collector_allowlist.rs` | The collector's attribute allowlist still covers what the code emits. | `make check` |
| `er-schema/tests/declared_names.rs` | The posterity gate, static half: any qualified `lake.main.<relation>` literal anywhere in the workspace must name a relation the S5 registry declares. | `make check` |
| `er-schema/tests/freeform_sql.rs` | The posterity gate for free-form lake SQL: after the reads migration (F0–F6) every persistent lake statement — read or write — is a declared entity statement, a plan-vector-tested generator, or … | `make check` |
| `er-schema/tests/span_boundaries.rs` | The posterity gate for blocking hops: no bare `spawn_blocking` in the control plane. | `make check` |
| `er-schema/tests/sql_span_pii.rs` | The PII boundary on spans, enforced rather than documented. | `make check` |
| `er-schema/tests/structured_logging.rs` | `er-server` logs through `tracing`, not `eprintln!`. | `make check` |
| `er-schema/tests/telemetry_layering.rs` | The telemetry layering rule: only a crate that owns an entry point may depend on `er-obs`. | `make check` |
| `er-server/tests/control_tasks_no_engine.rs` | A control task runs inside the dispatcher with the control plane's own credentials, so it must be STRUCTURALLY unable to run a stage: no file under `src/dags/control/` may name `er_cli::`, `dedupe_cor… | `make check` |
| `er-server/tests/egress_guard.rs` | The posterity gate for the outbound-URL guard. | `make check` |
| `er-server/tests/no_second_executor.rs` | The posterity gate for the one-executor rule (§11): the Kubernetes test double lives in `tests/`, and this scan is what keeps it there. | `make check` |
| `er-server/tests/pg_serial.rs` | Every `pg_*.rs` integration test must be ONE `#[tokio::test]`. | `make check` |

Every row above fails with the offending **file and line** in the
message. A finding that restates one is not a false positive — it is
worse, because it arrives with evidence and still wastes the cycle.

The `Runs in` column is the only reason to read a row twice. A gate in
`make check` has already spoken by the time a PR is reviewable, so its
subject is settled. A gate in the e2e tier self-skips without its tier
variable, so on a diff whose e2e leg has not reported yet its subject is
still open — raise it, and say which gate will confirm it.
<!-- END GENERATED: review-gates -->

Three more gates belong to this list but derive from a registry or from the
handlers rather than from source text, so the table above cannot see them:
`er-obs/tests/cardinality.rs` (a metric dimension that costs money),
`er-server/tests/dispatch_coverage.rs` (a `JobKind` with nowhere to go) and
`er-server/tests/response_envelopes.rs` (the per-route envelope ledger).

## The standing exclusions

Drop without scoring:

- **Anything a tool runs on its own.** The compiler, clippy, `cargo fmt`,
  `er-entity verify`, `tsc`, ESLint, `tflint`, `trivy`, `checkov`, the coverage
  floor, the axe gate, `cargo-deny`, `pnpm audit`. Assume every one of them runs.
  Do not build and do not run tests to find out.
- **Lines this diff did not modify.** A real bug on an untouched line is
  `🟣 Pre-existing` at most, capped, never blocking, and never phrased as
  something the author did.
- **General quality** — coverage, docs, naming, "consider extracting this" —
  unless a `CLAUDE.md` on a parent path of the file, or a house skill, states
  the rule. Then quote it.
- **A behaviour change the PR body says is intended.** Disagreeing with the
  intent is a conversation to have, not a finding to file.
- **Anything silenced in code with a reason** — an `#[allow]` with its
  justification comment, an eslint-disable with a note. The allow policy is
  `rust-conventions`', and the comment is the author's answer.
- **A count, version or threshold stated in prose.** That is a real defect class
  in this repo, and `skill-audit` owns it for skills. Outside skills, prefer the
  generated-block fix over a review comment.
- **Nitpicks a senior engineer would not raise out loud.** If the fix is "either
  spelling is fine", there is no finding.

## Security precedents

The bundled `/security-review` is the per-change security gate and `/pentest`
is the campaign; this skill reports a security issue only when it is also a
house-invariant issue. When one does come up, these precedents apply:

- No denial of service, rate limiting or resource exhaustion.
- No memory safety in safe Rust. `unsafe` without a `// SAFETY:` note is fair.
- No theoretical races without a concrete interleaving.
- No outdated-dependency CVEs — `cargo-deny` and Dependabot own them, and a
  supply-chain gate failing is a `ci-and-supply-chain` matter, not a review one.
- Test-only files, fixtures and docs are out of scope, **except** for PII.
- Environment variables and CLI flags are trusted input.
- React/TSX is not XSS without `dangerouslySetInnerHTML`.
- Client-side code owes no authorization check; the handler does. See
  `frontend-bff` for which guard a route handler owns.
- A finding needs a concrete source → sink path with a file and line per hop.

**The one addition for this product.** A new path that puts record-level PII
where it can be read back — a log line, a metric label, a span attribute, a
spill or temp directory on a shared volume, a committed fixture, an error
string, a URL query — is **always reportable**, even where a gate covers the
export path. Names and addresses are the payload here, not metadata.

## House precedents — correct things that read as defects

Each of these has been proposed as a fix at least once, and each would be a
regression.

| Looks wrong | Is correct because |
|---|---|
| `__dedupe__`-prefixed SQL stages, and Splink naming in provenance | the prefix is the stage contract and the provenance is deliberate history. Not cleanup. |
| `eprintln!` outside `er-server` | `er-entity check`'s diagnostics, `er … --json`, the stage line and `er-mcp`'s JSON-RPC stdout are a human or machine contract. Converting one breaks a test or corrupts a protocol. |
| `mode: partial` on a new statement | it skips the planner, not the parser. `MERGE` is always partial. |
| an error message that does not include its cause | Display must not interpolate its own `source`; reporters walk the chain. Gated. |
| `er_obs::logging::stage_record_line` having no callers | calling it with defaulted zeros would fill `run_stages` with numbers every dashboard would read as measurements. It lands with its writer. |
| schema-reconciliation spans being orphan roots | they are emitted before a run span exists. Correct today, and knowing it stops a "the trace spine is broken" report. |
| `/audit`, `/runs` and `/webhooks` returning bare arrays while `/jobs` and `/reviews` wrap | the two conventions are genuinely mixed and unifying them breaks every tenant reading the API. The ledger records it per route. |
| the frontend validating `traceparent` again, mirroring the Rust rule | a rejected header fails silently on the Rust side, so mirroring beats forwarding. |
| `--destructive` still being red | the single stated exception to the grayscale rule. |
| two WCAG deviations from stock shadcn | deliberate, documented in `frontend-design-system`. |
| an IAM exception in `infra/` with a comment | the deliberate pre-production exceptions are tracked and named. Re-reporting one adds nothing; `infra-platform` has the list. |
| the MVP builder admin key | an owner decision, permanent, with `DenyProd*` guards. |
| a NodePool with no Karpenter installation | EKS Auto Mode ships the controller; the CRDs are the interface. |

## Things not to ask the author for

- **A `REVIEW.md`.** There is deliberately none. The rules live in this skill's
  references, in `CLAUDE.md`, and in the gates — three places, each reviewable,
  rather than a fourth that nothing checks. The base-branch property the
  external pattern wants comes from reading these references at `$BASE`.
- **A rerun of a cancelled push-to-main run.** In a merge burst the concurrency
  group keeps one waiter and cancels it; rerunning cannot help. `land-pr` owns
  the procedure.
- **More assertions on a new structural gate.** Ask instead whether it was
  observed failing. A detector breaks by noticing nothing.
- **A performance claim without evidence.** `ducklake-performance` states what
  evidence is admissible. "This looks slow" is not a finding, and neither is a
  partition proposal without the experiment matrix.
- **A gate on the first occurrence** of a bug. One instance is an instance.
