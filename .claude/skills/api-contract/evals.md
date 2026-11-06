# Evals: api-contract

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Where a schema change starts — and whether this skill fires at all

**Query:** "The records table in the UI needs a `last_verified_at` column. Add
it to the API response type."

Must:

- Trigger `api-contract` **without it being named**. This phrasing is also
  prime `entity-framework` vocabulary, and `entity-framework` is the incumbent
  for anything containing the word *entity*, so watch which skill loads.
- Start in `entities/<name>/entity.yaml`, **not** in TypeScript.
- Walk the whole chain: `er-entity gen` → `er-openapi > openapi.json` →
  `pnpm gen:api` → fix call sites.
- Say both generated files are committed and must go in the same commit.

Must not:

- Edit `schema.d.ts` or declare a hand-written interface.
- Stop at `er-entity gen`. A correct-but-incomplete answer that never
  regenerates the document or the TypeScript is the failure this scenario
  exists to catch, and it is what a session that loaded only
  `entity-framework` produces.

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
- Run the `cargo` command from `rust/`, with the redirect relative to it.

Must not:

- Suggest updating the snapshot by hand or deleting the assertion.

## 3. Is this breaking?

**Query:** "I'm making the `email` column nullable on the golden record. That's
a loosening, so it's backward compatible, right?"

Must:

- Say no — "a property no longer required" is one of the four things
  `er-openapi-check` reports, because a consumer that relied on the field being
  present must now handle its absence.
- Note the detector is **advisory, blocking once an external org key exists**,
  so the warning should be acted on rather than ignored.

Credit for mentioning the entity `version` bump rule, and that lake changes
must be additive.

## 4. The two clients

**Query:** "I need to call the control plane from a React component. Import the
server client and pass the bearer."

Must:

- Refuse: the upstream client is `src/lib/bff/upstream.ts`, which is
  `server-only` and carries the bearer; a browser module uses
  `src/lib/api/client.ts`, which goes to the BFF same-origin.
- Note the transport differs even though the data shapes do not.

Must not:

- Name `src/lib/api/erserver.ts`. That module was deleted when the five
  hand-built upstream paths were consolidated; reviving it is the specific
  mistake this scenario guards.
- Propose a new `fetch` to `ERSERVER_BASE_URL` instead of `upstreamFetch`.

Credit for routing the boundary details to the frontend-bff skill.

## 5. Paths versus component schemas

**Query:** "Does `openapi.json` describe the request body for
`POST /v1/orgs/{org}/jobs`?"

Must:

- Say per-path operation docs are **not modelled**; what the document pins is
  the **component schemas**, generated from the entity structs.
- Say the document's `paths` is empty, so `schema.d.ts` types no route at all.

Must not:

- Claim any path-level coverage exists, or that a route is validated against
  the document.

Credit for warning against inventing a parallel hand-written description of the
operation in the frontend, and for naming what *does* pin the route shape
(`tools.json`, `envelopes.json`, the mock contract).

## 6. `wire: [json]` declares, it does not emit

**Query:** "My new entity declares `wire: [json]` in its generate block, so the
OpenAPI component schema comes out automatically, right?"

Must:

- Say **no**: `wire: [json]` records that the entity's payloads cross the JSON
  wire; the component schema is a hand-written `ToSchema` registration in
  `er-server/src/openapi.rs`.
- Say CI *will* stop you — `every_json_wire_entity_has_a_schema_or_a_declared_gap`
  derives the json-wire set from the catalog and fails the Rust test job until
  the entity is either registered or named in `JSON_WIRE_WITHOUT_SCHEMA`.
- Say registering means regenerating `openapi.json` and `schema.d.ts` in the
  same commit.

Must not:

- Claim the schema is emitted from the declaration.
- Claim nothing checks the registration, or that the gate is a count of
  registered schemas.

Credit for noting that registering is a public-API decision — the schema becomes
one `er-openapi-check` tracks for breaking changes — which is why the gate forces
the choice instead of making it.
