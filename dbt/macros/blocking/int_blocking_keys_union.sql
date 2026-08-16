{#
  S4.2 `int_blocking_keys_union()` -- the entire body of `int_blocking_keys`.

  S4.2 makes `configs/*.yaml` `blocking:` the single source of truth for candidate
  generation and names ONE generator over it,
  `src/er/matching/model.py::blocking_rules_from_config`, which produces both
  consumers from one set of `expr` strings: the Splink `block_on` rules, and the dbt
  var payload this macro renders. The payload travels under the var name
  `src/er/matching/model.py::BLOCKING_DBT_VAR` spells, which is `blocking` -- the S6
  block's own name, because the var IS that block rendered.

  Everything that decides which keys exist is therefore in the payload and nothing is
  in the model file: adding a `blocking:` entry to the config changes the compiled SQL
  and the emitted key set with no model edit, which is the drift M12 identified and
  the direction S4.2 makes normative.

  Each entry renders the S4.2 branch template verbatim:

      select '<key_type>' as key_type, <expr> as key_value,
             record_key, source_system, source_record_id
      from {{ ref('int_std_records') }}
      where <expr> is not null and <expr> <> ''

  Two properties of that rendering are load-bearing and neither is cosmetic.

  `expr` is emitted UNMODIFIED. Splink receives `block_on()` on the byte-identical
  string, and T-BLK-1 (S8.3) compares the DISTINCT canonicalised pair set this table
  yields against Splink's blocked pair set. A requoting, a normalisation or a
  whitespace fix here would make the two candidate sets diverge for a reason no test
  could attribute to it.

  The `where` clause is NOT rebuilt here. It is `entry['where']`, which is
  `NULL_EMPTY_PREDICATE` already rendered around this entry's `expr` by the generator
  -- the S4.2 NULL/empty policy is spelled once, in Python, and consumed verbatim on
  this side. Both halves of it matter: `is not null` alone would let the empty string
  block every record whose key expression concatenates a missing component, and
  Splink's `block_on` never joins on NULL, so the `<> ''` arm is what makes the dbt
  side agree with it.
#}

{#
  FALLBACK ONLY, and it is the same fallback rule `dbt_project.yml`'s `vars:` block
  states for every other var the models read at render time: S6 is the single source
  of truth, the CLI passes the payload on EVERY invocation
  (`src/er/dbt_runner.py::render_dbt_vars`), so the override always wins and a
  divergence from `configs/*.yaml` is invisible to the pipeline and is not a defect.

  It exists because S9.1 runs `dbt parse` on a runner with no config document and no
  `--vars`, and because `--select intermediate` is a selection any caller may run
  without one. What those two need is the SHAPE -- one entry per key type carrying
  `key_type`, `expr` and the rendered `where` -- and an empty list does not supply it:
  a model with no branches is not SQL, and an `accepted_values` domain with no members
  compiles to `not in ()`, which no database parses.

  It lives here rather than in `dbt_project.yml` because it is read by this macro and
  by `models/intermediate/schema.yml`'s domain check, and one fallback that both read
  cannot drift from itself.
#}
{% macro blocking_payload() -%}
{%- set fallback = [
    {'key_type': 'email_exact', 'expr': 'email',
     'where': "email is not null and email <> ''"},
    {'key_type': 'phone_exact', 'expr': 'phone_e164',
     'where': "phone_e164 is not null and phone_e164 <> ''"},
    {'key_type': 'name_postal', 'expr': "substr(family_name,1,4) || '|' || addr_postal",
     'where': "substr(family_name,1,4) || '|' || addr_postal is not null and substr(family_name,1,4) || '|' || addr_postal <> ''"},
    {'key_type': 'dob_name', 'expr': "birth_date || '|' || substr(given_name,1,3)",
     'where': "birth_date || '|' || substr(given_name,1,3) is not null and birth_date || '|' || substr(given_name,1,3) <> ''"}
] -%}
{#- `blocking` is `src/er/matching/model.py::BLOCKING_DBT_VAR`; the two ends of the
    var have to be the same string or the macro reads nothing. -#}
{{- return(var('blocking', fallback)) -}}
{%- endmacro %}

{#- The `accepted_values` domain of `int_blocking_keys.key_type`, for
    `models/intermediate/schema.yml`. Derived from the payload and never transcribed:
    S4.2 gives the config the sole say over which key types exist, and a literal list
    beside it would be the copy that stops agreeing. -#}
{% macro blocking_key_types() -%}
{{- return(blocking_payload() | map(attribute='key_type') | list) -}}
{%- endmacro %}

{% macro int_blocking_keys_union() -%}
{%- for entry in blocking_payload() -%}
{%- if not loop.first %}

union all

{% endif -%}
{{ blocking_keys_branch(entry) }}
{%- endfor -%}
{%- endmacro %}

{#- One payload entry as the S4.2 branch. The `key_type` literal is escaped for SQL
    the way `stg_<source>` escapes the config strings it embeds; S6.1 V7 has already
    rejected a document whose entries share one. -#}
{% macro blocking_keys_branch(entry) -%}
select '{{ entry['key_type'] | replace("'", "''") }}' as key_type, {{ entry['expr'] }} as key_value,
       record_key, source_system, source_record_id
from {{ ref('int_std_records') }}
where {{ entry['where'] }}
{%- endmacro %}
