# Board recipes

Copyable sequences for the board (`2 --owner athvin`). Every command here has
been run against the live board.

Each recipe derives the ids it needs in the same snippet, because an id pasted
from a previous session is the most common way these fail. See the skill body
for why, and for the error messages a stale id produces.

## Contents

- [Shell preamble](#shell-preamble)
- [Look up an id](#look-up-an-id)
- [Set a named single-select to a named option](#set-a-named-single-select-to-a-named-option)
- [Triage an issue onto the current iteration](#triage-an-issue-onto-the-current-iteration)
- [Read the board as a table](#read-the-board-as-a-table)
- [Reconcile the board against open issues](#reconcile-the-board-against-open-issues)
- [Find cards the automations left half-filled](#find-cards-the-automations-left-half-filled)
- [Clear a field](#clear-a-field)

## Shell preamble

Everything below assumes these two:

```sh
OWNER=athvin
NUM=2
PROJ=$(gh project view "$NUM" --owner "$OWNER" --format json --jq .id)
```

`PROJ` is re-derived rather than pasted. It is stable in practice, but deriving
it costs one call and removes the only id in these recipes that anyone would be
tempted to hard-code.

## Look up an id

```sh
# every field with its type
gh project field-list "$NUM" --owner "$OWNER" --format json \
  --jq '.fields[] | "\(.type)\t\(.id)\t\(.name)"'

# a named single-select: its field id and every option id
gh project field-list "$NUM" --owner "$OWNER" --format json \
  --jq '.fields[] | select(.name=="Status") | "field=\(.id)", (.options[] | "  \(.name) = \(.id)")'

# the item id for a given issue number  (--limit, or a big board truncates)
gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json \
  --jq '.items[] | select(.content.number==419) | .id'
```

`field-list` reports a coarse `type` (`ProjectV2Field`, `…SingleSelectField`,
`…IterationField`). The precise `dataType` that picks the `item-edit` flag —
`NUMBER` versus `DATE` versus `TEXT`, all of which are plain `ProjectV2Field` —
comes only from GraphQL:

```sh
gh api graphql -f query='query{user(login:"athvin"){projectV2(number:2){
  fields(first:30){nodes{
    ... on ProjectV2FieldCommon{name dataType} }}}}}' \
  --jq '.data.user.projectV2.fields.nodes[] | "\(.dataType)\t\(.name)"'
```

## Set a named single-select to a named option

The common case, by name at both ends, with no id written down:

```sh
set_select() {   # set_select <issue-number> <field-name> <option-name>
  local issue=$1 field=$2 option=$3
  local item fid oid
  item=$(gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json \
    --jq ".items[] | select(.content.number==$issue) | .id")
  fid=$(gh project field-list "$NUM" --owner "$OWNER" --format json \
    --jq ".fields[] | select(.name==\"$field\") | .id")
  oid=$(gh project field-list "$NUM" --owner "$OWNER" --format json \
    --jq ".fields[] | select(.name==\"$field\") | .options[] | select(.name==\"$option\") | .id")
  [ -n "$item" ] && [ -n "$fid" ] && [ -n "$oid" ] || { echo "lookup failed: item=$item fid=$fid oid=$oid" >&2; return 1; }
  gh project item-edit --id "$item" --project-id "$PROJ" --field-id "$fid" --single-select-option-id "$oid"
}
```

The emptiness check is the point. A `--jq` select that matches nothing yields an
empty string and exit 0, so without it a typo'd field name becomes
`--field-id ""`, and the error that comes back talks about the field rather than
the typo.

Setting `Status` to `Done` through this is the one call to think twice about —
see the skill body on "Auto-close issue".

## Triage an issue onto the current iteration

Four fields is four `item-edit` calls; there is no batch form. The four are
`Priority`, `Estimate`, `Iteration` and `Target date` — **not `Size`**, which is
left empty here. Filling it reintroduces the second, disagreeing estimate the
convention in the skill body exists to avoid.

```sh
ISSUE=123
WEEKS=3        # engineering-weeks remaining, which is what Estimate carries
ITEM=$(gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json \
  --jq ".items[] | select(.content.number==$ISSUE) | .id")

# field ids
F_PRIO=$(gh project field-list "$NUM" --owner "$OWNER" --format json --jq '.fields[]|select(.name=="Priority")|.id')
F_EST=$(gh project field-list  "$NUM" --owner "$OWNER" --format json --jq '.fields[]|select(.name=="Estimate")|.id')
F_ITER=$(gh project field-list "$NUM" --owner "$OWNER" --format json --jq '.fields[]|select(.name=="Iteration")|.id')
F_DUE=$(gh project field-list  "$NUM" --owner "$OWNER" --format json --jq '.fields[]|select(.name=="Target date")|.id')

# option id, by name — Priority is the only single-select of the four
O_P1=$(gh project field-list "$NUM" --owner "$OWNER" --format json \
  --jq '.fields[]|select(.name=="Priority")|.options[]|select(.name=="P1")|.id')

# the current iteration is iterations[0] — completed ones live in a separate array
O_ITER=$(gh api graphql -f query='query{user(login:"athvin"){projectV2(number:2){
  field(name:"Iteration"){... on ProjectV2IterationField{configuration{iterations{id}}}}}}}' \
  --jq '.data.user.projectV2.field.configuration.iterations[0].id')

gh project item-edit --id "$ITEM" --project-id "$PROJ" --field-id "$F_PRIO" --single-select-option-id "$O_P1"
gh project item-edit --id "$ITEM" --project-id "$PROJ" --field-id "$F_EST"  --number "$WEEKS"
gh project item-edit --id "$ITEM" --project-id "$PROJ" --field-id "$F_ITER" --iteration-id "$O_ITER"
gh project item-edit --id "$ITEM" --project-id "$PROJ" --field-id "$F_DUE"  --date 2026-12-01

# confirm — every write above was silent
gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json \
  --jq ".items[] | select(.content.number==$ISSUE)"
```

`Estimate` is a plain `ProjectV2Field` of `dataType` `NUMBER`, so it takes
`--number` and has no option id to derive — the single-select lookup that
`Priority` needs would return nothing for it.

Leave `Status` alone: "Item added to project" has already set it.

## Read the board as a table

```sh
gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json --jq '
  .items[] | [
    (.content.number // "draft"),
    .status,
    (.priority // "-"),
    (.estimate // "-"),
    (.iteration.title // "-"),
    (.["target date"] // "-"),
    .title
  ] | @tsv'
```

Custom fields are keyed by the field name lowercased with spaces intact, hence
`.["target date"]`. `iteration` is an object. A field never set is absent rather
than null, so `//` defaults are load-bearing.

## Reconcile the board against open issues

"Auto-add to project" means this should return nothing. When it returns
something, the automation was off or the issue predates it:

```sh
comm -23 \
  <(gh issue list --repo athvin/entity-resolution-engine --state open --limit 500 \
      --json number --jq '.[].number' | sort -n) \
  <(gh project item-list "$NUM" --owner "$OWNER" --limit 500 --format json \
      --jq '.items[] | select(.content.number) | .content.number' | sort -n)
```

`comm` needs both sides sorted the same way; `sort -n` on both, and do not
substitute `sort` on one side only.

## Find cards the automations left half-filled

The automations set `Status` and nothing else, so this is the real triage queue:

```sh
gh project item-list "$NUM" --owner "$OWNER" --limit 200 --format json --jq '
  .items[] | select(.priority == null or .estimate == null)
  | "\(.content.number // "draft")\t\(.status)\t\(.estimate // "-")\t\(.title)"'
```

Do not add `.size == null` to that predicate. `Size` is empty on every card by
convention, so including it matches the whole board and the queue stops
distinguishing anything — a half-filled card and a fully triaged one come back
alike. A missing key reads as `null` in `jq`, which is why `== null` works even
though an unset field is absent from the object rather than present as null.

## Clear a field

`--clear` replaces the type-specific flag and takes no value:

```sh
gh project item-edit --id "$ITEM" --project-id "$PROJ" --field-id "$F_DUE" --clear
```

Clearing `Status` takes a card out of every board-layout column, including the
one the automations would have put it in. That is rarely what is wanted.
