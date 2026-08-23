{#
  S4.6's five survivorship rules, one macro each, and the mandatory terminal element.

  Each rule is exactly one row of the S4.6 "Literal `ORDER BY` fragment" table. The
  fragments are emitted VERBATIM rather than compiled from the config, and that is the
  contract: a rule names columns the caller must present, and `golden_records.sql`
  (ER-088) is what presents them. Concretely, the relation handed to
  `survivorship_decision` must carry, per member row of an entity:

    entity_id            the partition
    value                the attribute's value for this row -- the member rows are in
                         LONG form, one row per (entity_id, attribute, value), which is
                         why `value` is a real column and carries no `<...>` in S4.6
                         while `<attr>_valid` does
    record_key           S5.0's scalar identity, and the terminal tiebreak
    source_system        the key into `sources`
    sources              MAP(VARCHAR, STRUCT(priority_rank INTEGER)) -- the config's
                         source table, so `sources[source_system].priority_rank` is a
                         real lookup and not a notation
    updated_at_source    nullable; `recency` COALESCEs it
    ingested_at          the `recency` fallback (a VOLATILE_COLUMNS member, S5.0 --
                         which is why ER-060 forbids a contest being decided by it)
    <attr>_valid         for `validated`; `int_std_records` carries `phone_valid` as
                         well as `email_valid`, so this is NOT special-cased to email

  **One table of terms, two consumers.** Every rule is defined once, in
  `survivorship_rule_terms`, as `(expression, direction)` pairs.
  `survivorship_order_by` renders them as an ORDER BY; `survivorship_decision`
  materialises the same expressions as named keys and compares them between the rank-1
  and rank-2 rows. Deriving both from one table is what makes it impossible for the
  ordering and its own explanation to disagree -- a rule that sorted on one expression
  and attributed on another would report the wrong deciding rule for exactly the ties
  it exists to explain.

  **Why the terms are a delimited string and not a list.** A dbt macro can `return()` a
  real list, but the ER-037 unit harness renders this tree under plain Jinja, where
  `return` does not exist. These macros therefore stay inside the intersection of the
  two dialects -- no `return`, no `{% do %}`, no `exceptions` -- so that one definition
  is exercised by the unit layer and shipped to dbt. The separators are chosen to be
  impossible inside an ORDER BY expression: a bare comma is not, which is the whole
  reason for them (`COALESCE(updated_at_source, ingested_at) DESC` contains one).
#}

{%- macro TERM_SEPARATOR() -%};;{%- endmacro -%}
{%- macro DIRECTION_SEPARATOR() -%}@@{%- endmacro -%}

{%- macro TERMINAL_TIEBREAK() -%}
record_key ASC
{%- endmacro -%}

{#- The name S5's closed `golden_lineage.rule` vocabulary uses when the terminal
    element decided, or when there was nothing to decide against. -#}
{%- macro TIEBREAK_RULE_NAME() -%}
tiebreak_deterministic
{%- endmacro -%}

{#-
  Rule -> its S4.6 terms, encoded as `expr@@dir;;expr@@dir`.

  `attribute` is substituted only where S4.6 writes `<attr>`. A rule that ignores it
  still takes it, so every rule has one calling convention and the chain renderer does
  not need to know which is which.

  An unknown rule fails the render. Under dbt that is a compilation error; under the
  unit harness's `StrictUndefined` the dict subscript raises too, and both messages
  name the offending rule, which is what AC7 asks for. The lookup is written as a
  subscript rather than an `{% if %}` chain precisely so the failure carries the name
  without this macro having to raise by hand in two dialects.
-#}
{%- macro survivorship_rule_terms(rule, attribute) -%}
  {%- set known = {
        'source_priority': 'sources[source_system].priority_rank@@ASC',
        'recency': 'COALESCE(updated_at_source, ingested_at)@@DESC',
        'frequency': 'count(*) OVER (PARTITION BY entity_id, value)@@DESC',
        'completeness': '(value IS NOT NULL)@@DESC;;length(value)@@DESC',
        'validated': '<attr>_valid@@DESC NULLS LAST',
      } -%}
  {%- if rule not in known -%}
    {#- Names the rule in the error, in both dialects. -#}
    {{ known['survivorship: unknown rule ' ~ rule ~ ' (S4.6 defines '
             ~ (known.keys() | join(', ')) ~ ')'] }}
  {%- endif -%}
  {{- known[rule] | replace('<attr>', attribute) -}}
{%- endmacro -%}

{#- One rule's terms rendered as the S4.6 fragment: `expr dir[, expr dir]`. -#}
{%- macro survivorship_rule_fragment(rule, attribute) -%}
  {%- set encoded = survivorship_rule_terms(rule, attribute) -%}
  {%- set rendered = [] -%}
  {%- for term in encoded.split(TERM_SEPARATOR()) -%}
    {%- set _ = rendered.append(term.split(DIRECTION_SEPARATOR()) | join(' ')) -%}
  {%- endfor -%}
  {{- rendered | join(', ') -}}
{%- endmacro -%}

{#- The five named entry points of the S4.6 table. -#}

{%- macro rule_source_priority(attribute) -%}
{{- survivorship_rule_fragment('source_priority', attribute) -}}
{%- endmacro -%}

{%- macro rule_recency(attribute) -%}
{{- survivorship_rule_fragment('recency', attribute) -}}
{%- endmacro -%}

{%- macro rule_frequency(attribute) -%}
{{- survivorship_rule_fragment('frequency', attribute) -}}
{%- endmacro -%}

{%- macro rule_completeness(attribute) -%}
{{- survivorship_rule_fragment('completeness', attribute) -}}
{%- endmacro -%}

{%- macro rule_validated(attribute) -%}
{{- survivorship_rule_fragment('validated', attribute) -}}
{%- endmacro -%}
