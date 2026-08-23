{#
  The chain concatenator and the deciding-rule attribution of S4.6.

  `survivorship_order_by(attribute, chain)` renders the chain's fragments in order,
  comma-joined, and terminates it with `record_key ASC`.

  **The terminal element is mandatory and appended exactly once.** S4.6 makes
  `record_key ASC` a MANDATORY terminal element of EVERY chain -- it is what makes each
  chain a total order, and it is load-bearing for T-INC-1 and T-GOLD-1, because without
  it the winner depends on physical row order, which differs between the touched-subset
  and the full-corpus materialisation. S6.1's config normalization ALREADY appends it,
  so a chain arriving here may or may not carry it and this macro must be idempotent:
  it strips any terminal element it finds and appends exactly one. Appending blindly
  would emit `record_key ASC, record_key ASC`, which sorts identically and is therefore
  the kind of defect that never shows up as a wrong answer -- only as a reviewer
  wondering which copy is authoritative.

  `survivorship_decision(attribute, chain, relation)` emits a SELECT returning one row
  per `entity_id`: the rank-1 winner's `value` and `record_key`, and the name of the
  rule that decided it.

  **The attribution contract, which ER-088 and ER-090 rely on.** The deciding rule is
  the FIRST rule in the chain whose sort key differs between the rank-1 and the rank-2
  row. When there is no rank-2 row -- a single candidate -- or when every rule's key
  ties, the rule is `tiebreak_deterministic`, which is the terminal element having
  decided. That vocabulary is closed by S5's `golden_lineage.rule` comment, so those
  are the only two kinds of answer this macro may give.

  Comparing rank-1 against rank-2 is enough, and comparing against the whole partition
  would be wrong: the chain is a total order, so the rule that separated the winner
  from the field is by definition the one that separated it from its nearest rival.

  **Why every key is materialised before ranking.** `frequency`'s term is itself a
  window function, and SQL does not allow a window function inside another window's
  ORDER BY. The keys are therefore computed in `_keyed` and the ranking orders on the
  aliases. The same structure is what makes the rank-1/rank-2 key comparison possible
  at all -- so the two requirements are met by one CTE rather than by two mechanisms
  that could drift.
#}

{%- macro survivorship_chain_without_terminal(chain) -%}
  {#- The chain's rule names, with any terminal element removed. Emitted as a
      delimited string because a plain-Jinja macro cannot return a list. -#}
  {%- set kept = [] -%}
  {%- for rule in chain -%}
    {%- if rule | trim != TERMINAL_TIEBREAK() | trim -%}
      {%- set _ = kept.append(rule | trim) -%}
    {%- endif -%}
  {%- endfor -%}
  {{- kept | join(TERM_SEPARATOR()) -}}
{%- endmacro -%}

{%- macro survivorship_order_by(attribute, chain) -%}
  {%- set encoded = survivorship_chain_without_terminal(chain) -%}
  {%- set fragments = [] -%}
  {%- if encoded -%}
    {%- for rule in encoded.split(TERM_SEPARATOR()) -%}
      {%- set _ = fragments.append(survivorship_rule_fragment(rule, attribute)) -%}
    {%- endfor -%}
  {%- endif -%}
  {%- set _ = fragments.append(TERMINAL_TIEBREAK()) -%}
  {{- fragments | join(', ') -}}
{%- endmacro -%}

{%- macro survivorship_decision(attribute, chain, relation='member_rows') -%}
  {%- set encoded = survivorship_chain_without_terminal(chain) -%}
  {%- set rules = encoded.split(TERM_SEPARATOR()) if encoded else [] -%}

  {#- Every rule's terms, aliased. `keys[i]` is rule `i`'s list of alias names, so the
      ORDER BY and the attribution CASE below read the same table in the same order. -#}
  {%- set projections = [] -%}
  {%- set ordering = [] -%}
  {%- set comparisons = [] -%}
  {%- for rule in rules -%}
    {%- set terms = survivorship_rule_terms(rule, attribute).split(TERM_SEPARATOR()) -%}
    {%- set differs = [] -%}
    {%- for term in terms -%}
      {%- set parts = term.split(DIRECTION_SEPARATOR()) -%}
      {%- set alias = '_k_' ~ rule ~ '_' ~ loop.index0 -%}
      {%- set _ = projections.append(parts[0] ~ ' AS ' ~ alias) -%}
      {%- set _ = ordering.append(alias ~ ' ' ~ parts[1]) -%}
      {%- set _ = differs.append('_w.' ~ alias ~ ' IS DISTINCT FROM _r.' ~ alias) -%}
    {%- endfor -%}
    {%- set _ = comparisons.append(
          'WHEN ' ~ (differs | join(' OR ')) ~ " THEN '" ~ rule ~ "'") -%}
  {%- endfor -%}
  {%- set _ = ordering.append(TERMINAL_TIEBREAK()) -%}

WITH _keyed AS (
    SELECT
        *{% if projections %},
        {{ projections | join(',\n        ') }}{% endif %}
    FROM {{ relation }}
),
_ranked AS (
    SELECT
        *,
        row_number() OVER (
            PARTITION BY entity_id
            ORDER BY {{ ordering | join(', ') }}
        ) AS _rn
    FROM _keyed
),
_top AS (
    SELECT * FROM _ranked WHERE _rn <= 2
)
SELECT
    _w.entity_id,
    _w.value,
    _w.record_key,
    CASE
        {# No rank-2 row means a single candidate: the terminal element decided by
           default, and S5's vocabulary spells that `tiebreak_deterministic`. #}
        WHEN _r.record_key IS NULL THEN '{{ TIEBREAK_RULE_NAME() }}'
        {% for comparison in comparisons %}{{ comparison }}
        {% endfor -%}
        ELSE '{{ TIEBREAK_RULE_NAME() }}'
    END AS rule
FROM _top AS _w
LEFT JOIN _top AS _r
       ON _r.entity_id = _w.entity_id
      AND _r._rn = 2
WHERE _w._rn = 1
{%- endmacro -%}
