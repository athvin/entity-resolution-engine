# Evals: er-server-control-plane

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Lake data in a handler

**Query:** "Add `GET /v1/orgs/{org}/golden-records/stats` returning a count of
golden records. Just query `lake.main.golden_records` in the handler."

Must:

- Refuse the inline SQL and name the posterity gate: `er-server/src` may contain
  **no `lake.main.<relation>` literal**, enforced by `freeform_sql.rs`.
- Route the work to declaring a statement under `entities/` (the
  entity-framework skill).

Credit for noting the aggregate needs an explicit `CAST(… AS BIGINT)` and a
nullable declaration, which is an entity-framework rule.

## 2. Cross-org status code

**Query:** "A viewer from org A requested org B's dashboard and got a 404. That
looks wrong — shouldn't it be 403?"

Must:

- Say **404 is correct and deliberate**: a 403 confirms the org exists, which
  is an enumeration oracle.
- State the split: 404 for cross-org scope, 403 for insufficient role within
  your own org.

Credit for mentioning 409 as the distinct lifecycle-state answer.

## 3. The lost claim

**Query:** "Our dispatcher logs show `claim` hitting a unique-constraint
violation under load. Should it retry, or propagate the error?"

Must:

- Say the violation is the **one-active-job-per-org constraint** doing its job,
  and it is already mapped to `Ok(None)` — not an error.
- Explain why propagating would be wrong: an `Err` from `claim` is fatal to the
  loop by design, so this would crash-loop the dispatcher under normal
  concurrency.

Must not:

- Propose application-level locking or an advisory lock to "fix" it.

## 4. Adding a pool

**Query:** "Connecting per request is wasteful. Add a connection pool."

Must:

- Flag that `er.actor` is set with `set_config(..., false)` and is therefore
  **session-scoped**, which is only safe because each request owns a
  connection.
- Say a pool requires making the actor transaction-scoped first, and name the
  consequence of not doing so: one request's actor attributed to another's
  writes, in the audit trail.

Credit for treating this as a real possible change with a prerequisite, rather
than refusing it outright.

## 5. A second Postgres test

**Query:** "Add a test to `pg_orgs.rs` covering the suspend→resume 409 path."

Must:

- Add a **phase to the existing single `#[tokio::test]`**, not a second test
  function, and cite `pg_serial.rs`.
- Mention the test self-skips without `ER_TEST_PG_DSN`.

Credit for noting the nasty failure mode: a second test passes locally and in
the default CI `test` job (both skipped) and only fails in the DuckLake e2e job.

## 6. A new request DTO

**Query:** "Add a request body struct for the new schedule endpoint."

Must:

- Include `#[serde(deny_unknown_fields)]` and explain it as the
  mass-assignment / typo defence.
- Build errors as `{"detail": …}` with the chain flattened via
  `errfmt::chain`, not `e.to_string()`.
