# DuckDB extensions

## Contents

- What is built in, and needs nothing
- What the named extensions add
- Two loading rules
- The engine version *is* the extension mechanism
- Testing
- Adding an extension-dependent comparison level

## What is built in, and needs nothing

The four built-in string metrics are named in the SKILL body. They cover the
comparison levels every committed model uses, so reach for an extension only
when a level genuinely needs something outside that set — which is rarer than
it looks, and is why step 1 of the checklist below is to check.

## What the named extensions add

| Extension | Adds |
|---|---|
| `splink_udfs` | phonetic functions: `soundex`, `double_metaphone` |

`splink_udfs` is one of the deliberate pieces of Splink provenance in the repo.
It is not a leftover to remove.

**`rapidfuzz` is no longer available** and was removed from this table at the
DuckDB 2.0 move. Its community-repository build for 2.0+ is a stub that throws
at init, pointing callers at a third-party repository (Query.Farm) instead — so
using it again would mean registering a new extension repository, which is a
supply-chain decision rather than a configuration one. Nothing in the repo
depended on it, and `rust/PERFORMANCE.md` measured its functions as the *same
speed* as DuckDB's built-ins, so there is no capability to recover. Note the
trap: the binary still returns HTTP 200, so checking availability proves
nothing — only loading it does.

## Two loading rules

Both rules — names validated as bare identifiers, and `install_extensions`
defaulting to `false` so only `LOAD` runs by default — are stated with their
reasoning in the SKILL body. Here is what they look like in the code, and the
two things the body leaves out.

The validation, which runs *before* any `INSTALL`/`LOAD` string is built:

```rust
if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
    // rejected
}
```

It is covered by an **offline** test (`rejects_invalid_extension_name`),
deliberately, so the guard stays verified without needing network.

The production bake points the engine at the baked directory:

```rust
SET extension_directory = '<dir>';
```

Baking also **pins the version**, which matters for anything that feeds a
score — a comparison level whose function changes behaviour between extension
builds moves match weights without any config change.

## The engine version *is* the extension mechanism

Every extension binary is published per engine version and cached per engine
version, at `<repository>/<version>/<platform>/<name>.duckdb_extension` and
`<extension_directory>/<version>/<platform>/…`. There is no compatibility
range: the version DuckDB reports for itself selects the binary, and a binary
built for another version is refused.

Two consequences that have each cost real time here:

- **A directory baked by a different DuckDB is silently ignored**, not an
  error — DuckDB just autoinstalls over the network again and the bake looks
  like it worked. This is why `er-server install-extensions` runs through the
  same statically linked engine that will later read the directory, rather than
  through a `duckdb` CLI.
- **If the engine reports a `-dev` version, nothing resolves at all.** DuckDB
  routes the URL through `source_id` instead of the version tag for any such
  label, and no binary is published under a source hash. `libduckdb-sys`'s
  `bundled` build does not stamp a release version onto the engine source it
  vendors, so the label has to be supplied by hand — see
  `rust/.cargo/config.toml`, and the `the_engine_reports_a_released_version`
  test that keeps it honest. Getting this wrong fails every extension fetch at
  once, which reads like an outage rather than a version problem.

So before moving the `duckdb` pin, check that every extension in
`BAKED_EXTENSIONS` — plus the community ones a config can ask for — actually
exists for the new version, on every platform the image targets. The core
repository is `extensions.duckdb.org`; community extensions live at
`community-extensions.duckdb.org` and are rebuilt per core version, so they can
lag a fresh DuckDB release.

## Testing

Loading a real extension needs a network `INSTALL`, so those tests are gated:

```sh
DEDUPE_E2E_EXTENSIONS=1 cargo test -p dedupe-core --test main extensions
```

CI runs this on the Rust **test shard**, not in the DuckLake e2e tier — it was
moved there because `cargo test --workspace` has already built `dedupe-core`'s
test binary by that point, so re-invoking it with the filter is run-only. The
variable is set for that one step rather than for the job, because `er-compile`
and `er-sql-check` gate tests behind the same name and a job-level `env` would
silently start running those on every PR. Exact job and required-check names are
in the `ci-and-supply-chain` skill's generated required-checks reference.

So the gap this leaves is **local**, not CI: with the variable unset a bare
`cargo test` silently skips the tier, and says nothing about whether your
extension loads. `make check-e2e` sets it.

## Adding an extension-dependent comparison level

1. Confirm the function is not already built in — the four metrics above cover
   more cases than people expect.
2. Add the extension name to the session options for the paths that need it,
   and make sure the deployment image carries the binary.
3. Add the comparator token in `er-compile` (see the er-matching skill) so a
   tenant can actually declare the level.
4. Decide how the SQL analyser will plan a call to it — a function the planner
   does not know needs a stub. See the sql-analysis skill; this is the step most
   likely to be missed, because the engine's own model SQL is golden-governed
   and will not complain.
5. Run the gated extension tier locally. A default test run will not exercise
   it.
