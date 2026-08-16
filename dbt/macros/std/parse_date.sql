{#
  S4.2 `parse_date(col, fmt)` -- the last row of the standardization macro table.

  It emits exactly ONE column, `birth_date DATE` (S5), and is used as a scalar: the
  caller supplies the alias, because `stg_<source>` and `int_std_records` name the
  column and this macro must not be a second declaration of that name.

  `fmt` is the source's own `sources.<name>.date_format` (S6), resolved at RENDER
  time and never read from a column: two sources deliver the same day under
  `%Y-%m-%d` and `%m/%d/%Y`, and the whole point of the per-source format is that
  the two produce the same DATE. `try_strptime` and not `strptime`, because a value
  the format does not describe is a record with no usable DOB, not a failed build --
  `raw_records` holds the source row verbatim (S4.1) and standardization is where a
  value stops being evidence.

  **Precision is computed here and never persisted (S4.2, explicit).** A `year`
  precision parse yields NULL, because a year-only DOB is not usable matching
  evidence, and v1 has no consumer for a precision column: no comparison level, no
  blocking key, no survivorship rule and no `golden_records` column reads one. Two
  distinct mechanisms produce that NULL, and both are needed:

  * a VALUE that does not fill the format -- `1985` under `%Y-%m-%d` -- is NULL
    because `try_strptime` refuses it. DuckDB's strptime matches the whole format,
    so a year-only value is rejected rather than zero-filled to January 1st.
  * a FORMAT that cannot express a day -- `%Y`, `%Y-%m` -- makes EVERY value it
    parses year- or month-precision, so the macro renders a typed NULL and does not
    call `try_strptime` at all. Without this branch a source declaring `date_format:
    "%Y"` would silently populate `birth_date` with January 1st of each year, which
    is a fabricated day that `exact` and `dob_same_year_month` (S4.3.1) would both
    read as evidence.

  `null_semantics` runs first, for the reason it runs anywhere: the sentinel
  vocabulary is absence, and `try_strptime` would reject `'unknown'` with the same
  NULL a genuinely absent value gets, making the two indistinguishable at the point
  where one is a data problem and the other is not.
#}

{#- The tokens that put a day in the parsed value: day of month in either width, or
    day of year. A format carrying none of them cannot yield a `day` precision, so
    `date_precision` reports the coarser answer and `parse_date` renders NULL. -#}
{% macro DAY_TOKENS() -%}
%d,%-d,%j
{%- endmacro %}

{#- Month tokens, in every spelling DuckDB's strptime accepts. Only used to tell
    `month` from `year`; neither yields a value, so the distinction exists for the
    reader and for a future S5.1 additive precision column, not for the SQL. -#}
{% macro MONTH_TOKENS() -%}
%m,%-m,%b,%B
{%- endmacro %}

{% macro date_precision(fmt) -%}
{%- set format = fmt | replace('%%', '') -%}
{%- if DAY_TOKENS().split(',') | select('in', format) | list -%}
day
{%- elif MONTH_TOKENS().split(',') | select('in', format) | list -%}
month
{%- else -%}
year
{%- endif -%}
{%- endmacro %}

{% macro parse_date(col, fmt) -%}
{%- set present = null_semantics(lowercase_trim(col)) | trim -%}
{%- if date_precision(fmt) | trim != 'day' -%}
cast(null as date)
{%- else -%}
{#- The format is a literal, not a bound parameter: it comes from the S6 document
    at render time, and embedded quotes are doubled the way every other rendered
    literal in this tree doubles them. -#}
cast(try_strptime({{ present }}, '{{ fmt | replace("'", "''") }}') as date)
{%- endif -%}
{%- endmacro %}
