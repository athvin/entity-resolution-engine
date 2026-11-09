---
name: api-contract
description: Explains the generated API contract and its gaps — entity schemas, OpenAPI emission, TypeScript generation, drift checks and breaking-change detection — for the configurable deduplication API and its frontend. Use when adding or changing an endpoint's request or response shape, documenting external-client data transfer, checking public API readiness, fixing stale OpenAPI or TypeScript artifacts, or deciding whether a schema change is breaking. For product priorities see project-board; for entity declarations see entity-framework; for handler and auth mechanics see er-server-control-plane; for the browser/server boundary see frontend-bff.
---

# The API contract

## Product intent and current coverage

OpenAPI is the customer integration contract for configurable deduplication:
dataset configuration, data submission, execution and result retrieval. Design
those operations for a customer using their own systems. Salesforce is the first
commercial adapter; SFTP and other transports must reuse the same contracts.
For product prioritization use `project-board`.

This is the intended boundary. The generated document currently pins component
schemas and source-derived operations between the Rust backend and TypeScript
frontend; typed responses, query parameters and consumer coverage remain gaps. A schema snapshot passing does not
prove that an external client can configure a dataset or transfer data. Changes
to the public surface owe documented auth, errors, retry semantics and an
executable customer workflow, as well as generated types.

The current generation chain is:

```
entities/*/entity.yaml
   │   er-entity gen
   ▼
er_entities::<entity>::<Name>Wire  (public columns; derives utoipa::ToSchema)
   │   a hand-maintained subset is registered in openapi.rs's ApiDoc
   ▼
cd rust && cargo run -p er-server --bin er-openapi
   ▼
frontend/src/lib/api/openapi.json     internal, pinned by a Rust snapshot test
   │   pnpm gen:api  (openapi-typescript)
   ▼
frontend/src/lib/api/schema.d.ts      committed, byte-compared in CI

--audience public emits openapi.public.json, independently snapshot-pinned
```

A schema change therefore starts in `entities/`, not in a TypeScript interface.
And the chain ends at `schema.d.ts` — read *What is generated and not yet
consumed* below before assuming a call site is typed against the backend.

## The two commands, in order

```sh
# 1. backend → the document. The cargo workspace root is rust/, so the
#    redirect is relative to it rather than to the repo root.
cd rust
cargo run -q -p er-server --bin er-openapi -- --audience internal > ../frontend/src/lib/api/openapi.json
cargo run -q -p er-server --bin er-openapi -- --audience public > ../frontend/src/lib/api/openapi.public.json

# 2. document → the TypeScript types
cd ../frontend && pnpm gen:api
```

Both documents and the internal TypeScript output are **committed** together
with the entity change. Never hand-edit these artifacts, and never edit `schema.d.ts` to make the
frontend compile — that silently decouples the two sides, which is the exact
failure the contract exists to prevent.

## What pins each step

The snapshot test lives in `er-server`, so **either stale OpenAPI snapshot fails a Rust
test job**, not a frontend one. If backend CI fails on an OpenAPI snapshot after
you changed an entity, you have found step 1, not a bug.

Which gate catches which mistake, and where it fails, is the table in
[references/changing-a-schema.md](references/changing-a-schema.md). It names
jobs by their workflow key; the status-check *context* a job reports under is
often a different string, and the generated map between the two is
`ci-and-supply-chain/references/required-checks.md`.

## Registration is manual; catalog guards enforce the decision

`ApiDoc` references the generated JSON wire structs' `ToSchema` **directly**, so a
registered schema follows `entities/` with no hand-written duplicate. That is
the part that is pinned.

What is *not* derived is which entities are registered. The list is written by
hand in `er-server/src/openapi.rs` and covers the control-plane tables plus the
produced golden marts rather than everything under `entities/`; the pipeline's
intermediate entities are deliberately absent.

**`wire: [json]` emits `<Name>Wire` with public columns only.** Its schema
name stays `<Name>` via `#[schema(as = <Name>)]`; the storage row has no
`ToSchema`. Convert rows through `From<&Name>`. `visibility: internal`
governs omission from the wire type and TypeScript binding, with V19 guarding
secret-shaped columns. Declaring JSON does not add the entity to `ApiDoc`. What
reconciles the two is
`every_json_wire_entity_has_a_schema_or_a_declared_gap`: it derives the
json-wire set from the catalog, subtracts the registered schemas, and requires
the difference to equal the `JSON_WIRE_WITHOUT_SCHEMA` backlog in that file. So
a **new** `wire: [json]` entity fails the Rust test job until someone either
registers its `ToSchema` or adds it to that list with a reason.

The founding backlog was registered in #900; #862 records the
[per-entity decisions and response-projection caveats](references/schema-registration-decisions.md).
Read `JSON_WIRE_WITHOUT_SCHEMA` and the catalog guards for the current gaps.
Registration is a public-API decision rather than a formality, because the
component becomes a documented schema that `er-openapi-check` then tracks for
breaking changes. A sibling test holds the other direction: nothing may be
registered that does not declare `wire: [json]`. Neither guard proves that a
handler returns the complete entity wire shape or that an operation references
that component.

## What is generated and not yet consumed

The operation contract is generated and gated, with deliberate limits:

- **Paths and operations come from source.** `openapi.rs` builds utoipa paths
  from `er_api_spec::operations`: the shared router walk, adjacent handler docs,
  signatures and response constructors. Every registration must
  resolve; unsupported request types or unclassifiable responses fail generation.
  Path parameters, summaries, role requirements and body media/schema are emitted.
  HTTP bearer auth documents tenant `erk_` keys and the separate operator token;
  public health operations have no security requirement. Internal attempt routes
  use a separate `attemptAuth` scheme for projected ServiceAccount or per-attempt
  fallback tokens. The public audience drops operator roles and `/v1/internal/*`
  from that same derivation; it has no separate route allowlist. TypeScript still
  uses the complete internal document. OpenAPI 3.1 permits
  role names in a non-OAuth security requirement.
- **Responses remain envelope-level.** Success uses `2XX`; bare and typed JSON
  values stay unconstrained rather than guessing arrays from variable names.
  Object envelopes name observed properties, pages describe items/cursor, and
  Arrow uses its actual media type. Local response helpers contribute write
  envelopes. Typed payloads and query parameters remain later work; neither a
  generated path nor its TypeScript type proves those details are complete.
  The detail-shaped application error is documented separately from framework
  request-extractor and infrastructure rejections.
- **No frontend module imports `schema.d.ts`.** `openapi-fetch` sits in
  `package.json` with no call sites. The backend types that do reach the
  frontend come from `er-entity gen`'s `src/lib/entities/bindings/`, imported by
  the query hooks and the golden-page decoder; `bffFetch<T>` takes its type from
  whichever caller invokes it.

So the gates prove the document and the TypeScript agree with the Rust structs.
They prove nothing about whether frontend code agrees with either. Do not cite a
green `schema.d.ts` check as evidence that a call site is type-safe against the
backend.

## Route moves change the generated contract

The document uses actual router paths (including the slash forms behind action
aliases), so a route move changes both OpenAPI and generated TypeScript. The
operation floor and per-registration coverage gate reject an empty or partial
walk. The older adapters still pin complementary views:

| What | Pinned by |
|---|---|
| the axum router's `.route(...)` calls in `er-server/src/lib.rs` | nothing — hand-written, and the source the rest derive from |
| `rust/crates/er-mcp/tools.json` | `er-server/tests/mcp_manifest.rs`: byte-compared, **and** every routed handler must carry its doc comment, so a new route fails until it is described |
| `frontend/src/lib/api/envelopes.json` | `er-server/tests/response_envelopes.rs`: derived from the handlers and byte-compared |
| the mock in `frontend/tests/mock-erserver/server.mjs` | `er-server/tests/mock_contract.rs`, **one direction only**: the mock may serve no route the router lacks. A router route the mock does not serve passes |

The mock gate remains one-directional; generated paths do not prove mock coverage.

The source derivation lives in `er-api-spec`: one balanced router walk plus
handler-doc/request-body and success-envelope classifiers. Its functions take
source strings and do not depend on `er-server` or perform I/O. The three server
tests above supply the real source and retain their parser floors, missing-route
failures and byte-for-byte artifact checks. Their views intentionally differ:
the MCP gate checks described methods, the envelope gate classifies GET handlers,
and the mock gate erases parameter names and uses the server's action-path
normalizer. Extend the shared derivation when adding a supported source shape;
do not restore a private router scanner in a consumer. OpenAPI also parses Rust
syntax for function boundaries, qualified request structs and local response
helpers, with module sources supplied explicitly by the caller. It preserves
serde field defaults, optionality and unknown-field policy. Existing MCP and GET
ledger bytes retain their established parser views; their limitations do not
justify guessing a new OpenAPI request schema.

## Breaking changes

`er-openapi-check --audience internal|public` compares the selected document
with its baseline. Alongside removed schemas/properties, property type changes
and dropped response requirements, it reports removed operations, added path
parameters and newly required request properties (including optional-to-required
fields and local references). New operations and optional properties are
compatible. This is bounded detection, not complete schema-language compatibility:
query inputs, arbitrary schema compositions and response payload narrowing remain
outside it.

It is a step in the Rust `entities` job, on **pull requests only** (it diffs
against `origin/main`, so a push to `main` has no base and it examines nothing),
and compares both audiences. A missing baseline is explicitly reported as initial
publication, never compared against the other audience. It is **advisory**, so it cannot
fail the job. The
product requirement is to have **blocking compatibility checks ready before the
first external production org key is issued**. The deployed-baseline work is
tracked on the live board; this requirement does not claim it has shipped.
Pass `--strict` to enforce the detector's current coverage locally.

So an advisory warning here is not noise to be ignored — it is a preview of what
will block later, and the right time to decide whether the change is really
compatible.

The detected changes, known limits, and what
to do when a change genuinely must break:
[references/changing-a-schema.md](references/changing-a-schema.md).

## Error bodies are part of the contract too

Handlers return `{"detail": "…"}`. The frontend BFF parses that shape, so it is
a contract even though it is not in the component schemas. Changing it is a
frontend-visible break. See the er-server-control-plane skill.

## Common workflows

**Added a column to an entity.** Regenerate the entity artifacts
(`er-entity gen`), then the document, then the TypeScript. Three generated
layers, one source. `make check` catches the first; the Rust snapshot test the
second; the frontend job the third.

**Added a whole entity.** The same, plus a decision if it declares
`wire: [json]`: register its `ToSchema` in `openapi.rs`, or add it to
`JSON_WIRE_WITHOUT_SCHEMA` there. The Rust test job fails until you do one of
the two, so this is the rare case the gate makes you choose rather than letting
you forget.

**The frontend needs a field the backend does not return.** The work is
backend-first: the column goes in the entity, the statement projects it, the
document regenerates. Do not widen a TypeScript type to cover a field that does
not exist.

**CI says `schema.d.ts` is stale but I did not touch the frontend.** Expected —
a backend schema change requires regenerating the frontend types in the same
commit. Run both commands above.

**A type looks wrong in the frontend.** Check `openapi.json` first. If the
document is right and `schema.d.ts` is wrong, regenerate. If the document is
wrong, the entity or the statement's `returns:` is wrong, and that is an
entity-framework problem.
