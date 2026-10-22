# Injection

Classic SQLi is *mostly designed out here* by the entity framework — which is exactly why the
remaining injection surface is narrow, specific, and easy to miss.

## Contents
- Why ordinary SQLi is (mostly) not the risk
- Config-as-code: the real SQL surface
- The `format!` generators: audit every path has a guard
- CSV formula injection
- Path traversal via config source names
- The `:action` path-rewriter
- The MAX_KEYS delimiter bypass
- How to prove / how to lock

## Why ordinary SQLi is (mostly) not the risk

Every declared statement is parameter-bound (`duckdb::params![...]` / `client.query(SQL, &[...])`).
The only textual substitution in the generated crate is the DuckLake snapshot pin
(`.replace("{snapshot}", ...)`), and that value is always a Rust `i64` — not injectable. Two posterity
gates keep it that way: `freeform_sql.rs` (no `query_strings` caller outside the engine core, no
`lake.main.<x>` literal in `er-server/src`) and `declared_names.rs`. **Do not** try to "find SQLi" in
declared statements; spend the effort on the four surfaces below.

## Config-as-code: the real SQL surface

Three config fields are **raw SQL by design** (`er-config/src/lib.rs`):
- `blocking[].expr` — "a raw scalar SQL expression over standardized columns"
- `training.deterministic_rules: Vec<String>`
- `training.em_blocking_rules: Vec<String>`

They flow into `dedupe-core/src/blocking.rs`, `blocking_analysis.rs`, `linker.rs`. Publish-time
analysis (`er-server/src/config_check.rs` `config_reference_problems`) checks only *column references*
in `blocking[].expr` — **not subqueries, functions, or side effects, and the two `training` lists get
no analysis at all.** `er-config::load_config_str` does structural validation only ("Full V1–V16
validation is not yet ported").

Reachability: `POST /v1/orgs/{org}/config/versions` + publish needs `Role::Admin`; `OPERATOR_ONLY_BLOCKS`
gates `training` behind operator. So **an admin can author arbitrary blocking SQL**; `training` SQL
needs operator. Treat config publish as an authenticated code-execution surface gated by role — the
test (#4) is to extend `config_check.rs` to parse `deterministic_rules`/`em_blocking_rules` and reject
non-column constructs (subquery, function call, `;`, comment), then add unit cases.

## The `format!` generators: audit every path has a guard

`er-golden` and `er-standardize` assemble SQL by interpolation:
- `er-golden/src/records.rs` — `format!("max(r.{a}) filter (where l.attribute = '{a}') as {a}")`
- `er-golden/src/survivorship.rs` — alias construction
- `er-standardize/src/lib.rs` — `json_extract_string(payload, '...')` with source column names

Mitigations exist but are **uneven** — the audit is "does every config-derived string reaching a
`format!` pass through exactly one of these?":
- attribute names constrained to the static `CANONICAL_ATTRIBUTES` list
- `master.rs` `quote_literal` (doubles `'`)
- `er-standardize` routes source column names through `single_quote` into a *literal* position
- `survivorship.rs` `alias_key` sanitizes to `[A-Za-z0-9_]`

Any config-derived value reaching an *identifier* position without `alias_key`-style sanitizing is a
finding. These are generator functions — the test home is a plan-vector case in
`er-sql-check/src/stage.rs` (feed a hostile config value, assert it is sanitized/rejected, not planned
verbatim).

## CSV formula injection

`er-server/src/read.rs` `merge_plans_csv`/`csv_cell` escapes `"`, `,`, `\n` but **not** a leading
`=`, `+`, `-`, `@`, tab, or CR. Exported cells are tenant-ingested names and emails, reachable at
`GET /v1/orgs/{org}/merge-plans?format=csv` (Viewer) and the BFF segments/export. A record named
`=HYPERLINK(...)` executes in a spreadsheet. Fix: prefix a cell beginning with a formula
metacharacter with `'` (or wrap). Test (#5): co-located unit in `read.rs`.

## Path traversal via config source names

`er-server/src/dispatcher.rs` builds `format!("{drop_root}/{source}.csv")` from config `sources` keys
with no sanitization — `../../etc/passwd` style traversal, or absolute paths. Fix: validate source
names at config load (reject `/`, `..`, leading `/`, NUL). Test (#8): unit in `dispatcher.rs` on the
path builder + a reject case in `config_check.rs`.

## The `:action` path-rewriter

`er-server/src/lib.rs` `rewrite_action_path` runs in an outer `fallback_service` **before routing**,
mapping `.../x:verb` → `.../x/verb` for 8 `ACTION_VERBS`. Route-confusion primitive: fuzz ids
containing `:`, encoded `%3A`, double colons, and path params that themselves end in a verb. Test (#7):
co-located unit or `tests/http.rs` (no pg) asserting no verb-suffixed id smuggles into a privileged
sub-path and that a legitimate colon-in-id is untouched.

## The MAX_KEYS delimiter bypass

`er-server/src/read.rs` `keys_from_query` caps at 50 keys but doesn't strip `\u{1f}`, the delimiter it
later joins on in the IN-list idiom. A single `key=` value containing `\u{1f}` expands into N keys —
parameterized, so not injection, just a cap bypass (resource lever). Test (#6): co-located unit in
`read.rs`.

## How to prove / how to lock

| Claim | PoC | Test |
|---|---|---|
| `deterministic_rules` runs arbitrary SQL | publish a config whose rule is a subquery; it's accepted | `config_check.rs` unit (#4) |
| `format!` identifier unsanitized | hostile attribute/source name planned verbatim | `stage.rs` plan-vector |
| CSV formula executes | name `=cmd()` round-trips uninjected | `read.rs` unit (#5) |
| Path traversal | source name `../x` builds an out-of-root path | `dispatcher.rs` unit (#8) |
| Route confusion | `id:cancel` reaches the cancel sub-path | `http.rs` unit (#7) |
| Key cap bypass | one `key=` with `\u{1f}` yields >50 keys | `read.rs` unit (#6) |
