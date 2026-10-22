# DuckDB extensions

## What is built in, and needs nothing

The core string metrics ship with DuckDB and need no extension:

- `jaro_winkler_similarity`
- `levenshtein`
- `damerau_levenshtein`
- `jaccard`

These cover the comparison levels the committed models use. Reach for an
extension only when a level genuinely needs something outside this set.

## What the named extensions add

| Extension | Adds |
|---|---|
| `rapidfuzz` | token-based ratios — `token_sort`, `token_set`, `partial` — for comparisons where word order or substring containment matters |
| `splink_udfs` | phonetic functions: `soundex`, `double_metaphone` |

`splink_udfs` is one of the deliberate pieces of Splink provenance in the repo.
It is not a leftover to remove.

## Two loading rules

**Names are validated as bare identifiers.** Every extension name must be ASCII
alphanumeric or `_`, checked *before* any `INSTALL`/`LOAD` is built:

```rust
if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
    // rejected
}
```

The name is interpolated into SQL, so this check is the only thing standing
between a configuration value and `LOAD <arbitrary SQL>`. Keep it if you touch
that code. It is covered by an offline test
(`rejects_invalid_extension_name`) precisely so the guard is verified without
needing network.

**`install_extensions` defaults to `false`.** `LOAD <ext>` always runs;
`INSTALL <ext> FROM community` runs only when the flag is on, because it needs
network.

Production does **not** install at runtime. Extension binaries are baked into
the image and `extension_directory` points at them:

```rust
SET extension_directory = '<dir>';
```

A container that installs from the community registry at startup needs egress
to it and fails closed when the registry is unreachable or slow — turning a
registry outage into an application outage. Baking them in also pins the
version, which matters for anything that feeds a score.

## Testing

Loading a real extension needs a network `INSTALL`, so those tests are gated:

```sh
DEDUPE_E2E_EXTENSIONS=1 cargo test -p dedupe-core --test extensions
```

CI runs this tier in the DuckLake e2e job, not in the default `test` job. The
split matters when adding an extension-dependent comparison: a green
`cargo test --workspace` says nothing about whether your extension loads.

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
