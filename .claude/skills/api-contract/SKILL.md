---
name: api-contract
description: Explains the typed API contract between the Rust control plane and the Next.js frontend — the utoipa `ApiDoc` whose component schemas come from the generated entity structs, the `er-openapi` binary that emits `openapi.json`, the Rust snapshot test that pins it, `openapi-typescript` regenerating `schema.d.ts`, the CI drift gate on both, and the `er-openapi-check` breaking-change detector. Use when adding or changing an endpoint's request or response shape, when the OpenAPI document or the generated TypeScript types are stale in CI, when a frontend type does not match the backend, or when deciding whether a schema change is breaking. For declaring the entity behind a schema see entity-framework; for handler and auth mechanics see er-server-control-plane; for which client a module may import and the browser/server line see frontend-bff.
---

# The API contract

One document is the contract between the Rust backend and the TypeScript
frontend, and **no part of it is hand-written**:

```
entities/*/entity.yaml
   │   er-entity gen
   ▼
er_entities::<entity>  (derives utoipa::ToSchema)
   │   a hand-maintained subset is registered in openapi.rs's ApiDoc
   ▼
cd rust && cargo run -p er-server --bin er-openapi
   ▼
frontend/src/lib/api/openapi.json     committed, pinned by a Rust snapshot test
   │   pnpm gen:api  (openapi-typescript)
   ▼
frontend/src/lib/api/schema.d.ts      committed, byte-compared in CI
```

A schema change therefore starts in `entities/`, not in a TypeScript interface.
And the chain ends at `schema.d.ts` — read *What is generated and not yet
consumed* below before assuming a call site is typed against the backend.

## The two commands, in order

```sh
# 1. backend → the document. The cargo workspace root is rust/, so the
#    redirect is relative to it rather than to the repo root.
cd rust && cargo run -q -p er-server --bin er-openapi > ../frontend/src/lib/api/openapi.json

# 2. document → the TypeScript types
cd frontend && pnpm gen:api
```

Both outputs are **committed**, and both belong in the same commit as the entity
change. Never hand-edit either, and never edit `schema.d.ts` to make the
frontend compile — that silently decouples the two sides, which is the exact
failure the contract exists to prevent.

## What pins each step

The snapshot test lives in `er-server`, so **a stale `openapi.json` fails a Rust
test job**, not a frontend one. If backend CI fails on an OpenAPI snapshot after
you changed an entity, you have found step 1, not a bug.

Which gate catches which mistake, and where it fails, is the table in
[references/changing-a-schema.md](references/changing-a-schema.md). It names
jobs by their workflow key; the status-check *context* a job reports under is
often a different string, and the generated map between the two is
`ci-and-supply-chain/references/required-checks.md`.

## Registration is manual, and the test named after it does not check it

`ApiDoc` references the generated entity structs' `ToSchema` **directly**, so a
registered schema follows `entities/` with no hand-written duplicate. That is
the part that is pinned.

What is *not* derived is which entities are registered. The list is written by
hand in `er-server/src/openapi.rs` and covers the control-plane tables plus the
produced golden marts rather than everything under `entities/`; the pipeline's
intermediate entities are deliberately absent.

**`every_generated_entity_schema_is_registered` does not enforce its own name.**
It asserts the *size* of `ApiDoc`'s registration list, so it fires when a
registration is added or removed and is blind to `entities/`. Add an entity,
never register its schema, and the size is unchanged and the test is green. No
gate catches that, so registering a new entity's schema is a step you have to
remember — put it in the same change as the entity.

## What is generated and not yet consumed

Two gaps are easy to mistake for working machinery, because the artifacts around
them really are generated and gated:

- **No path is described.** `ApiDoc` declares no `paths(...)` group, so the
  document carries `"paths": {}` and `schema.d.ts` opens with
  `export type paths = Record<string, never>`. The type contract was what a
  split backend/frontend most needed; operations ride on top later.
- **No frontend module imports `schema.d.ts`.** `openapi-fetch` sits in
  `package.json` with no call sites. The backend types that do reach the
  frontend come from `er-entity gen`'s `src/lib/entities/bindings/`, imported by
  the query hooks and the golden-page decoder; `bffFetch<T>` takes its type from
  whichever caller invokes it.

So the gates prove the document and the TypeScript agree with the Rust structs.
They prove nothing about whether frontend code agrees with either. Do not cite a
green `schema.d.ts` check as evidence that a call site is type-safe against the
backend.

## A route move is invisible to this gate

When the tenant surface moved under `/v1/orgs/{org}/datasets/{ds}/…`,
`openapi.json` did not change by a single byte — it carries no paths. What pins
the route shape is elsewhere, and the four are not equally strong:

| What | Pinned by |
|---|---|
| the axum router's `.route(...)` calls in `er-server/src/lib.rs` | nothing — hand-written, and the source the rest derive from |
| `rust/crates/er-mcp/tools.json` | `er-server/tests/mcp_manifest.rs`: byte-compared, **and** every routed handler must carry its doc comment, so a new route fails until it is described |
| `frontend/src/lib/api/envelopes.json` | `er-server/tests/response_envelopes.rs`: derived from the handlers and byte-compared |
| the mock in `frontend/tests/mock-erserver/server.mjs` | `er-server/tests/mock_contract.rs`, **one direction only**: the mock may serve no route the router lacks. A router route the mock does not serve passes |

Do not read a clean `openapi.json` diff as evidence that a path change is
contained.

## Breaking changes

`er-openapi-check` diffs the document this build emits against `main`'s and
reports four things: a schema removed, a property removed, a property whose
`type` changed, and a property no longer required. Additions are **not**
reported — adding an optional property is compatible.

It is a step in the Rust `entities` job, on **pull requests only** (it diffs
against `origin/main`, so a push to `main` has no base and it examines nothing),
and it is **advisory**: every stage is `|| true` or `|| exit 0`, so it cannot
fail the job. The
stated intent is that it becomes **blocking once the first external org key
exists**, at which point the release pipeline runs it with `--strict`. Pass
`--strict` to enforce it locally.

So an advisory warning here is not noise to be ignored — it is a preview of what
will block later, and the right time to decide whether the change is really
compatible.

The four with worked examples, the two edges the list does not cover, and what
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

**Added a whole entity.** The same, plus registering its `ToSchema` in
`openapi.rs` by hand if the control lane serves it as JSON — nothing will tell
you that you forgot.

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
