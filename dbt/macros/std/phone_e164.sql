{#
  S4.2 `phone_e164(col)` -- the phone half of standardization.

  Like `email_norm`, this is used in a SELECT list and not as a scalar: it expands
  to the TWO projections S5 names on `stg_<source>` and `int_std_records`,
  `phone_e164` and `phone_valid BOOL`. `phone_valid` is what S6.1 V4 requires
  before `validated` may appear in the phone survivorship chain, and `phone_e164`
  is the S6 `phone_exact` blocking expression -- which is why the rendered value
  carries digits and a leading `+` and nothing else. `base_10`'s drifted-phone
  trap (S8.2) is the executable form of that: `(415) 555-0132`, `415-555-0132` and
  `+14155550132` are one number and must produce one key.

  No parsing library. S2.1's rule is that a dependency is a row in the pin table,
  so this is pure Jinja over DuckDB string functions; `phonenumbers` would be a
  spec amendment, not a macro edit.

  The region comes from `standardization.phone_default_region` (S6, V16) and is
  resolved HERE, at render time, so the rendered SQL carries no region literal at
  all -- change the var and the same macro source renders different SQL. v1
  renders the NANP only. S6.1 V16 admits any ISO alpha-2, and for any other region
  this macro yields NULL rather than a guessed rendering: a wrong `+1` on a
  British number would not merely be a bad value, it would be a blocking key that
  silently joins two unrelated records.

  `phone_valid` is three-valued, and the three cases mean what `email_valid`'s do
  (S4.6 orders `<attr>_valid DESC NULLS LAST`):

    NULL   no evidence -- NULL, blank, or a `null_semantics` sentinel.
    true   a non-empty value that renders as E.164.
    false  a non-empty value that does not. `phone_e164` is NULL here too, but the
           `false` records that we saw something and rejected it.

  A value that already carries a `+` is treated as E.164 and never re-prefixed:
  its digits are preserved verbatim and it never reaches the region branch, so
  `+442071838750` cannot come back as `+1442071838750`. The length window is
  E.164's own (max 15 digits, country code never leading-zero); a `+` value
  outside it is a rejected observation, not a number to guess at.
#}
{% macro phone_e164(col) -%}
{%- set region = var('standardization')['phone_default_region'] | upper -%}
{#- Sentinels first: `null_semantics` owns that vocabulary and this macro repeats
    none of it. `lowercase_trim` inside it is what makes `  4155550132 ` and a
    padded `'N/A'` arrive in the one spelling the sentinel list is written in. -#}
{%- set present = null_semantics(lowercase_trim(col)) -%}
{%- set digits = "regexp_replace(" ~ present ~ ", '[^0-9]', '', 'g')" -%}
{%- set is_e164 = "starts_with(" ~ present ~ ", '+')" -%}
{%- set e164_ok = "length(" ~ digits ~ ") between 7 and 15 and not starts_with(" ~ digits ~ ", '0')" -%}
{%- set rendered %}
case when {{ is_e164 }}
       then case when {{ e164_ok }} then '+' || {{ digits }} end
{%- if region == 'US' %}
     when length({{ digits }}) = 10 then '+1' || {{ digits }}
     when length({{ digits }}) = 11 and starts_with({{ digits }}, '1') then '+' || {{ digits }}
{%- endif %}
end
{%- endset -%}
{{ rendered }} as phone_e164,
case when {{ present }} is null then null else {{ rendered }} is not null end as phone_valid
{%- endmacro %}
