# Evals: api-contract

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Where a schema change starts

**Query:** "The records table in the UI needs a `last_verified_at` column. Add
it to the API response type."

Must:

- Start in `entities/<name>/entity.yaml`, **not** in TypeScript.
- Walk the chain: `er-entity gen` → `er-openapi > openapi.json` → `pnpm gen:api`
  → fix call sites.
- Say both generated files are committed and must go in the same commit.

Must not:

- Edit `schema.d.ts` or declare a hand-written interface.

Credit for noting any `returns: entity` statement must now project the new
column (E003).

## 2. A Rust CI failure that is really about frontend types

**Query:** "The Rust `test` job is failing on
`openapi_json_matches_the_committed_document` but I only changed an entity. Is
the test broken?"

Must:

- Say the test is correct: the committed `openapi.json` is stale.
- Give the fix as both commands — regenerate the document **and** `pnpm
  gen:api` — because the frontend job will fail next otherwise.

Must not:

- Suggest updating the snapshot by hand or deleting the assertion.

## 3. Is this breaking?

**Query:** "I'm making the `email` column nullable on the golden record. That's
a loosening, so it's backward compatible, right?"

Must:

- Say no — "a property no longer required" is one of the four things
  `er-openapi-check` reports, because a consumer that relied on the field being
  present must now handle its absence.
- Note the detector is **advisory today, blocking once an external org key
  exists**, so the warning should be acted on rather than ignored.

Credit for mentioning the entity `version` bump rule, and that lake changes
must be additive.

## 4. The two clients

**Query:** "I need to call the control plane from a React component. Import the
client from `src/lib/api/erserver.ts`."

Must:

- Refuse: `erserver.ts` is **server-only** and carries the bearer; a browser
  module uses `src/lib/api/client.ts`, which goes to the BFF same-origin.
- Note the generated types are shared but the transport is not.

Credit for routing the boundary details to the frontend-bff skill.

## 5. Paths versus component schemas

**Query:** "Does `openapi.json` describe the request body for
`POST /v1/orgs/{org}/jobs`?"

Must:

- Say per-path operation docs are **not modelled yet**; what the document pins
  is the **component schemas**, generated from the entity structs.
- Not claim full path-level coverage exists.

Credit for warning against inventing a parallel hand-written description of the
operation in the frontend.
