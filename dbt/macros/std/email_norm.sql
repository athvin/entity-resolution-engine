{#
  S4.2 `email_norm(col)` -- the email half of standardization.

  Used in a SELECT list, not as a scalar: it expands to the TWO projections S5
  names on `stg_<source>` and `int_std_records`, `email` and `email_valid BOOL`.

  Every setting comes from the `standardization` var that
  `src/er/dbt_runner.py::render_dbt_vars` builds from the S6 document. Nothing is
  hard-coded here -- not the placeholder list, not the plus-addressing switch --
  because S6 is the single source of truth and a literal in this file would be a
  second one that no validator can see.

  Placeholder nulling lives here and in no other macro (S4.2, explicit):
  `null_semantics` owns the sentinel vocabulary, which contains no email address,
  and `base_10` carries `test@test.com` as the executable proof of the split.

  `email_valid` is three-valued on purpose, and the three cases are not
  interchangeable (S4.6's `validated` rule orders `<attr>_valid DESC NULLS LAST`):

    NULL   no evidence -- the input was absent, or was a configured placeholder.
           A nulled placeholder MUST NOT present as `false`, which would rank it
           below a record that has no email at all.
    true   a non-empty value that parses as an address.
    false  a non-empty value that does not. `email` is NULL in this case too --
           an unparsable string is not an address -- but the `false` records that
           we saw something and rejected it.

  The validity pattern is the S6.1 V16 pattern (`src/er/config/schema.py`)
  transcribed into RE2, so a value that V16 accepts as a placeholder is a value
  this macro also considers well-formed. It is written with POSIX classes rather
  than `\s` and `\.` so that no backslash has to survive both Jinja's string
  escaping and DuckDB's literal parsing.
#}
{% macro email_norm(col) -%}
{%- set std = var('standardization') -%}
{%- set normalized = lowercase_trim(col) | trim -%}
{%- if std['email_strip_plus_addressing'] -%}
{#- Only the first `+...` run of the local part, and only before the `@`. -#}
{%- set normalized = "regexp_replace(" ~ normalized ~ ", '[+][^@]*@', '@')" -%}
{%- endif -%}
{%- set valid = "regexp_full_match(" ~ normalized ~ ", '[^@[:space:]]+@[^@[:space:]]+[.][^@[:space:]]+')" -%}
{%- if std['email_placeholders'] -%}
{%- set literals = std['email_placeholders'] | map('replace', "'", "''") | join("', '") -%}
{%- set is_placeholder = normalized ~ " in ('" ~ literals ~ "')" -%}
{%- else -%}
{%- set is_placeholder = "false" -%}
{%- endif -%}
case when {{ is_placeholder }} or not {{ valid }} then null else {{ normalized }} end as email,
case when {{ normalized }} is null or {{ is_placeholder }} then null else {{ valid }} end as email_valid
{%- endmacro %}
