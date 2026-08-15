{#
  S4.2 `null_semantics(col)` -- map the sentinel vocabulary to NULL.

  Two constraints from S4.2, both of which this file is the only enforcement of:

  * The vocabulary is declared ONCE, by `NULL_SENTINELS`, and every consumer
    renders that macro rather than repeating the list. S4.2 lists it in mixed
    case (`'NULL'`, `'N/A'`), so the comparison is made against the lowered,
    trimmed value and the vocabulary is spelled lowercase here -- otherwise
    `'null'` and `'n/a'` would survive standardization and reach the matcher as
    literal evidence.
  * It handles the sentinel vocabulary and NOTHING else. Placeholder email
    addresses belong to `email_norm` and to no other macro, which is why
    `'test@test.com'` passes through this macro unchanged; `base_10` carries that
    address precisely so the division of labour is executable rather than
    asserted.

  A non-sentinel is returned VERBATIM -- not trimmed, not lowered. This macro
  decides presence, `lowercase_trim` decides spelling, and a macro that quietly
  did both would make the two indistinguishable at the call site.
#}
{% macro NULL_SENTINELS() -%}
'', 'null', 'n/a', '-', 'unknown'
{%- endmacro %}

{% macro null_semantics(col) -%}
case when lower(trim({{ col }})) in ({{ NULL_SENTINELS() }}) then null else {{ col }} end
{%- endmacro %}
