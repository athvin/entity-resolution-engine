# Changing a schema, end to end

## Contents

- The checklist
- Which gate catches which mistake
- What counts as breaking
- Making a genuinely breaking change
- The frontend side
- Anti-patterns

## The checklist

Copy this and work through it. The order matters — each step's input is the
previous step's output — and every `cargo` command runs from `rust/`, the
workspace root.

```
- [ ] 1. Change the definition in entities/<name>/entity.yaml (and any
         statement whose `returns:` must now project the new column)
- [ ] 2. cd rust && cargo run -q -p er-entity-cli -- gen --root ..
- [ ] 3. cd rust && cargo run -q -p er-entity-cli -- verify --root ..
- [ ] 4. For a NEW entity the control lane serves as JSON: register its
         ToSchema by hand in rust/crates/er-server/src/openapi.rs and update
         the expected count in its test. No gate reports that you skipped this
- [ ] 5. cd rust && cargo run -q -p er-server --bin er-openapi \
            > ../frontend/src/lib/api/openapi.json
- [ ] 6. cd frontend && pnpm gen:api
- [ ] 7. Fix the Rust and TypeScript call sites that no longer compile
- [ ] 8. make check   (and make frontend if the UI changed)
- [ ] 9. Commit the definition AND every regenerated artifact together
```

Step 9 is not a formality. The gates compare committed output against freshly
generated output, so a commit that carries the definition without the artifacts
fails CI, and one that carries artifacts without the definition is
unreproducible.

## Which gate catches which mistake

Jobs below are named by their **workflow key**. The status-check context a job
reports under is often a different string, and the generated map between the two
is `ci-and-supply-chain/references/required-checks.md`.

| Mistake | Caught by | Where it fails |
|---|---|---|
| definition changed, artifacts not regenerated | `er-entity verify` | Rust `entities` job / `make check` |
| a `returns: entity` statement no longer projects every column | **E003** | same |
| `openapi.json` stale | `openapi_json_matches_the_committed_document` | Rust `test` job |
| `schema.d.ts` stale | `pnpm gen:api` + `git diff --exit-code` | frontend `check` job |
| a registration added or removed without updating the expected size | `every_generated_entity_schema_is_registered` | Rust `test` job |
| **a new entity never registered at all** | **nothing** | — |
| the change removes or retypes something | `er-openapi-check` | Rust `entities` job — **advisory**, prints only, pull requests only |

Note the split: most of these fail on the **backend** even though they are about
frontend types. A Rust CI failure mentioning OpenAPI is almost always step 5 or
6 missing.

The two registration rows are the ones to read twice.
`every_generated_entity_schema_is_registered` asserts the *size* of `ApiDoc`'s
hand-written list rather than a set difference against `entities/`, so it cannot
see an entity that was never registered — which is the gate its name promises.
It does fire the moment you add a registration, because the expected size then
disagrees; that is step 4's second half.

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

The detector is advisory until the first external consumer, so until then the
discipline has to come from you.

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

Two clients, and they are not interchangeable:

| Client | Runs | Talks to |
|---|---|---|
| `src/lib/bff/upstream.ts` | server-only (`import "server-only"`) | the Rust control plane, with the bearer |
| `src/lib/api/client.ts` | browser | the BFF, same-origin only |

`upstream.ts` is the single place the BFF builds an upstream request, and that
is the point of it: `traceparent` and the action header have to be on *every*
upstream call, which is not something five hand-built call sites can promise. A
browser module must never import it — see the frontend-bff skill, which covers
the `server-only` marker and the gates that enforce the boundary.

Neither client imports `schema.d.ts`. `bffFetch<T>` takes its type from the
caller, and where that is a generated type at all it comes from `er-entity
gen`'s `src/lib/entities/bindings/`.
`docs/entity-framework-design.md` has the control lane typed through
`schema.d.ts` and `openapi-fetch`; until that is wired, regenerating the types
cannot break the frontend build, so the compiler will **not** list the call
sites a backend change affects. Find them by hand.

## Anti-patterns

- **Declaring a TypeScript interface that mirrors a backend type.** It will
  drift silently. Import the generated binding from `src/lib/entities/`.
- **Building a new path to the control plane** rather than calling
  `upstreamFetch`. A second path is how a header that must be on every upstream
  request ends up on all but one; the module this consolidated
  (`src/lib/api/erserver.ts`) was deleted precisely because a dead path is one
  the next person revives.
- **Adding a path-level response shape in the frontend because the document
  does not describe operations.** It does not describe them *yet*; inventing a
  parallel description is the thing the contract exists to avoid.
