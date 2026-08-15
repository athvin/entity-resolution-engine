{#
  S4.2 `address_parse(cols)` -- the componentizer behind the `AddressParser`
  interface, versioned by `versions.address_parser_version` (S6).

  It expands to the SIX projections S5 names on `stg_<source>` and
  `int_std_records`. Only three of them are parsed: S6 states that `addr_city`,
  `addr_region` and `addr_postal` map directly from their own source columns, so
  they are `lowercase_trim`ped pass-throughs here and an `address_line` change can
  never move them. The single `address_line` of V11 is the only input the grammar
  below reads (S8.2: each source carries exactly one address column).

  The three pass-through arguments carry a `null` default so the S8.1 macro
  harness, which binds ONE input column, can render this macro over an address
  line alone. A dbt model passes all four; the default is inert, not a fallback.

  **The grammar is declared once per side, and the two sides are held together by
  a test, not by good intentions.** `src/er/std/address.py::RegexV1Parser` is the
  Python oracle of this same grammar; `tests/unit/dbt/test_address.py` asserts
  that the three patterns below are the byte-identical strings that module
  compiles AND that both implementations agree on every committed case. That is
  the T-BLK-1 shape applied to standardization: two implementations of one rule
  drift silently unless something compares them.

  The v1 grammar, over the normalized line:

  1. Normalize. `lowercase_trim` (NFC, lower, trim, `''` -> NULL), then `.` and
     `,` are removed so `st.` and `st` agree and `P.O. Box` reaches the rejection
     below in one spelling, then internal whitespace collapses to one space.
  2. `null_semantics` LAST, for the reason `name_norm` states: applied earlier,
     punctuation stripping could manufacture a sentinel out of a value the first
     pass let through, and a second pass would then answer differently.
  3. A PO box is REJECTED outright, not parsed. It has no number/street/unit
     decomposition, and guessing one would put a box number in `addr_number`
     where a comparison level would read it as a street address.
  4. The unit is a trailing `<keyword> <designator>` phrase, or a `#<designator>`;
     anchored at the end, so exactly one phrase is ever taken and re-parsing the
     rendered line reproduces it.
  5. The number is a leading `<digits>[letter]` token, so `221b` is a number and
     `5th` is not.
  6. The street is what remains.

  **All three components are NULL unless the street is (S4.2, and the reason the
  fixture generator emits only patterns this parser handles).** A line with no
  street -- a bare number, a unit with nothing to attach it to, a PO box -- is
  unparsable, and an unparsable address yields NULL components rather than a
  partial parse with the number folded into the street.
#}

{#- The unit phrase, anchored at the end of the line. Group 2 is the unit itself;
    the leading `(^| )` is matched so a replace removes the separator with it. -#}
{% macro ADDRESS_UNIT_PATTERN() -%}
(^| )((apt|apartment|unit|ste|suite|fl|floor|rm|room) [a-z0-9-]+|#[a-z0-9-]+)$
{%- endmacro %}

{#- A PO box in every spelling the punctuation strip leaves: `po box`, `p o box`
    (from `P. O. Box`) and `post office box`. -#}
{% macro ADDRESS_PO_BOX_PATTERN() -%}
^(p ?o|post office) box( .*)?$
{%- endmacro %}

{#- The house number: digits, optionally one letter (`221b`), then a boundary. -#}
{% macro ADDRESS_NUMBER_PATTERN() -%}
^([0-9]+[a-z]?)( |$)
{%- endmacro %}

{% macro address_parse(address_line_col, city_col='null', region_col='null', postal_col='null') -%}
{%- set stripped = "regexp_replace(" ~ lowercase_trim(address_line_col) | trim ~ ", '[.,]', '', 'g')" -%}
{%- set collapsed = "nullif(trim(regexp_replace(" ~ stripped ~ ", '[[:space:]]+', ' ', 'g')), '')" -%}
{%- set vocabulary = null_semantics(collapsed) | trim -%}
{%- set line = "case when regexp_full_match(" ~ vocabulary ~ ", '" ~ ADDRESS_PO_BOX_PATTERN() ~ "')"
               ~ " then null else " ~ vocabulary ~ " end" -%}
{%- set unit = "nullif(regexp_extract(" ~ line ~ ", '" ~ ADDRESS_UNIT_PATTERN() ~ "', 2), '')" -%}
{%- set rest = "trim(regexp_replace(" ~ line ~ ", '" ~ ADDRESS_UNIT_PATTERN() ~ "', ''))" -%}
{%- set number = "nullif(regexp_extract(" ~ rest ~ ", '" ~ ADDRESS_NUMBER_PATTERN() ~ "', 1), '')" -%}
{%- set street = "nullif(trim(regexp_replace(" ~ rest ~ ", '" ~ ADDRESS_NUMBER_PATTERN() ~ "', '')), '')" -%}
case when {{ street }} is null then null else {{ number }} end as addr_number,
{{ street }} as addr_street,
case when {{ street }} is null then null else {{ unit }} end as addr_unit,
{{ null_semantics(lowercase_trim(city_col) | trim) | trim }} as addr_city,
{{ null_semantics(lowercase_trim(region_col) | trim) | trim }} as addr_region,
{{ null_semantics(lowercase_trim(postal_col) | trim) | trim }} as addr_postal
{%- endmacro %}
