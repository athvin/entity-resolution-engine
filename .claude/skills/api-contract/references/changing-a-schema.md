# Changing a schema, end to end

## Contents

- The checklist
- Which gate catches which mistake
- What counts as breaking
- Making a genuinely breaking change
- The frontend side
- Anti-patterns

## The checklist

Copy this and work through it. The order matters: each step's input is the
previous step's output.

```
- [ ] 1. Change the definition in entities/<name>/entity.yaml (and any
         statement whose `returns:` must now project the new column)
- [ ] 2. cd rust && cargo run -q -p er-entity-cli -- gen --root ..
- [ ] 3. cd rust && cargo run -q -p er-entity-cli -- verify --root ..
- [ ] 4. cargo run -p er-server --bin er-openapi > frontend/src/lib/api/openapi.json
- [ ] 5. cd frontend && pnpm gen:api
- [ ] 6. Fix the Rust and TypeScript call sites that no longer compile
- [ ] 7. make check   (and make frontend if the UI changed)
- [ ] 8. Commit the definition AND every regenerated artifact together
```

Step 8 is not a formality. The gates compare committed output against freshly
generated output, so a commit that carries the definition without the artifacts
fails CI, and one that carries artifacts without the definition is
unreproducible.

## Which gate catches which mistake

| Mistake | Caught by | Where it fails |
|---|---|---|
| definition changed, artifacts not regenerated | `er-entity verify` | Rust `lint` job / `make check` |
| a `returns: entity` statement no longer projects every column | **E003** | same |
| new entity, no component schema registered | `every_generated_entity_schema_is_registered` | Rust `test` job |
| `openapi.json` stale | `openapi_json_matches_the_committed_document` | Rust `test` job |
| `schema.d.ts` stale | `pnpm gen:api` + `git diff --exit-code` | frontend `check` job |
| the change removes or retypes something | `er-openapi-check` | Rust `lint` job — **advisory**, prints only |

Note the split: three of these fail on the **backend** even though they are
about frontend types. A Rust CI failure mentioning OpenAPI is almost always
step 4 or 5 missing.

## What counts as breaking

`er-openapi-check` reports exactly four things:

| Reported | Example |
|---|---|
| a schema removed | an entity deleted, or its `ToSchema` unregistered |
| a property removed | a column dropped |
| a property whose `type` changed | `varchar` → `bigint` |
| a property no longer required | a non-null column made nullable |

Additions are **not** reported, because adding an optional property is
backward-compatible for a consumer that ignores unknown fields.

Two notes on the edges:

- "No longer required" is reported because a consumer that relied on a field
  always being present now has to handle its absence. That is a real break even
  though it loosens the schema.
- Making an optional property **required** is a break for *producers* but is
  not in the reported set. Think about direction, not just about which way the
  constraint moved.

## Making a genuinely breaking change

The detector is advisory now and blocking once an external consumer exists.
While it is advisory, the discipline has to come from you.

1. **Decide whether it is really necessary.** An added nullable column is not a
   breaking change and is almost always the better shape.
2. If the entity itself changes incompatibly (a type change, a rename, a drop),
   bump the entity's `version`. An added nullable column is not a version bump.
3. Update every consumer in the same change — the frontend is in the same repo,
   so there is no excuse for leaving it broken.
4. Say in the commit message that the API contract changed incompatibly, and
   what a consumer must do. The advisory output scrolls past in a log; the
   commit message does not.
5. For the lake, remember changes must be **additive** — it has no defaults and
   is rebuilt by the pipeline. A breaking lake change needs a hand-written
   migration path, and `er-entity migrate` refuses breaking diffs on the
   control store with `ERR_SCHEMA_BREAKING`.

## The frontend side

`schema.d.ts` feeds two clients, and they are not interchangeable:

| Client | Runs | Talks to |
|---|---|---|
| `src/lib/api/erserver.ts` | server-only | the Rust control plane, with the bearer |
| `src/lib/api/client.ts` | browser | the BFF, same-origin only |

The generated types are shared; the transport is not. A browser module must
never import the server client — see the frontend-bff skill, which covers the
`server-only` marker and the gates that enforce the boundary.

When regenerating types breaks the frontend build, that is the contract doing
its job: the compiler is listing the call sites a backend change affects. Fix
them rather than widening the type.

## Anti-patterns

- **Hand-editing `openapi.json` or `schema.d.ts`.** Both are regenerated and
  byte-compared; the edit will be reverted by the next gate, and in the
  meantime the two sides disagree.
- **Declaring a TypeScript interface that mirrors a backend type.** It will
  drift silently. Import from `schema.d.ts`.
- **Widening a frontend type to cover a field the backend does not send.** The
  field needs to exist in the entity first.
- **Adding a path-level response shape in the frontend because the document
  does not describe operations.** It does not describe them *yet*; inventing a
  parallel description is the thing the contract exists to avoid.
- **Ignoring the advisory breaking-change output** because it does not fail the
  build. It is a preview of a gate that becomes blocking.
