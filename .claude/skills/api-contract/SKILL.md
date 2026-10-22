---
name: api-contract
description: Explains the typed API contract between the Rust control plane and the Next.js frontend — the utoipa `ApiDoc` whose component schemas come from the generated entity structs, the `er-openapi` binary that emits `openapi.json`, the Rust snapshot test that pins it, `openapi-typescript` regenerating `schema.d.ts`, the CI drift gate on both, and the `er-openapi-check` breaking-change detector. Use when adding or changing an endpoint's request or response shape, when the OpenAPI document or the generated TypeScript types are stale in CI, when a frontend type does not match the backend, or when deciding whether a schema change is breaking. For declaring the entity behind a schema see entity-framework; for handler and auth mechanics see er-server-control-plane.
---

# The API contract

One document is the contract between the Rust backend and the TypeScript
frontend, and **no part of it is hand-written**:

```
entities/*/entity.yaml
   │   er-entity gen
   ▼
er_entities::<entity>  (derives utoipa::ToSchema)
   │   referenced by openapi.rs's ApiDoc
   ▼
cargo run -p er-server --bin er-openapi
   ▼
frontend/src/lib/api/openapi.json        committed, pinned by a Rust snapshot test
   │   pnpm gen:api  (openapi-typescript)
   ▼
frontend/src/lib/api/schema.d.ts         committed, byte-compared in CI
   │
   ▼
openapi-fetch client in src/lib/api/erserver.ts
```

A schema change therefore starts in `entities/`, not in a TypeScript interface.

## The two commands, in order

```sh
# 1. backend → the document
cargo run -p er-server --bin er-openapi > frontend/src/lib/api/openapi.json

# 2. document → the TypeScript types
cd frontend && pnpm gen:api
```

Both outputs are **committed**. Both are eslint-ignored, because they are
generated. Never hand-edit either, and never edit `schema.d.ts` to make the
frontend compile — that silently decouples the two sides, which is the exact
failure the contract exists to prevent.

## What pins each step

| Artifact | Gate | Failure message |
|---|---|---|
| `openapi.json` | the Rust unit test `openapi_json_matches_the_committed_document` | tells you to run the `er-openapi` command and then `pnpm gen:api` |
| `schema.d.ts` | the frontend `check` job: `pnpm gen:api` then `git diff --exit-code -- src/lib/api/schema.d.ts` | the step is "Generated API types are not stale" |
| schema coverage | `every_generated_entity_schema_is_registered` | a generated entity exists with no component schema |

The snapshot test lives in `er-server`, so **a stale `openapi.json` fails the
Rust `test` job**, not the frontend one. If backend CI fails on an OpenAPI
snapshot after you changed an entity, you have found step 1, not a bug.

The coverage test is the one that catches the quiet mistake: adding an entity
and forgetting to register its schema. The document carries one component
schema per generated entity — control entities plus the produced golden marts.

## Component schemas are the contract; paths are not yet

`ApiDoc` references the generated entity structs' `ToSchema` **directly**, so
the document follows `entities/` with no hand-written duplicate. That is the
part that is pinned.

Per-path operation docs are deliberately **not** modelled yet. The type
contract was what a split backend/frontend most needed, and paths ride on top
later. So:

- Do not expect request/response *operations* to be described in the document.
- Do expect every entity-shaped payload to be.

A consequence for the frontend: `openapi-fetch` gives you typed *schemas*, and
route paths are still typed from whatever the document does carry — check
before assuming a path is validated.

## Breaking changes

`er-openapi-check` compares the document this build emits against a base and
reports:

- a schema removed,
- a property removed,
- a property whose `type` changed,
- a property no longer required.

Additions are **not** reported — adding an optional property is compatible.

It runs in the Rust `lint` job and is **advisory today**: it prints and does not
fail, because there is no external consumer yet. The stated intent is that it
becomes **blocking once the first external org key exists**. Pass `--strict` to
enforce it locally.

So an advisory warning here is not noise to be ignored — it is a preview of
what will block later, and the right time to decide whether the change is
really compatible.

The full workflow, including the frontend side and what to do when a change
genuinely must break:
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
