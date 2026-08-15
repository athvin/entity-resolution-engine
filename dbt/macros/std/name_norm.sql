{#
  S4.2 `name_norm(col)` -- `lowercase_trim`, punctuation strip, diacritic fold.

  S4.2's macro row reads as one macro emitting three columns; it is two. This one
  is a SCALAR expression, applied to `given_name` and to `family_name` alike; the
  `name_variants LIST(VARCHAR)` array and its symmetry guarantee live in
  `name_variants`, which builds itself on top of this macro.

  The steps compose in the order below, and the order is load-bearing:

  1. `lowercase_trim` -- NFC, lower, trim, `''` -> NULL. It comes first because
     every later step is defined over the NFC spelling, and because folding an
     accent before composing it would fold whichever spelling happened to arrive.
  2. Diacritic fold, re-composed. `strip_accents` decomposes to NFD and drops the
     combining marks, so its result is NOT guaranteed to be NFC -- a script whose
     NFD form carries no combining marks (Hangul) comes back decomposed. The
     output of this macro is hashed by `std_hash` (T-STD-1, S8.3) and compared by
     every comparison level, so it is re-normalized rather than left in whatever
     form the fold produced. That is also what makes the macro idempotent, since
     a second pass would otherwise re-compose what the first left decomposed.
  3. Punctuation, pinned rather than left to taste: `-` and `_` become a single
     space and `.` `'` `’` `,` are removed, so `O'Brien` and `OBrien` normalize
     alike, as do `Mary-Jane` and `Mary Jane`. Splitting on the two classes is
     the whole point -- removing a hyphen would join `mary` and `jane`, and
     replacing an apostrophe with a space would split `obrien`.
  4. Internal whitespace collapses to one space and the result is trimmed, so a
     value cannot carry a spelling difference no comparison level can see.
  5. `null_semantics` LAST, not first. Two reasons. Its comparison is made
     against the lowered, trimmed value, so a raw sentinel is still caught here;
     and running it last is what makes the macro idempotent unconditionally --
     applied first, `'NULL.'` would normalize to `'null'`, which a second pass
     would then map to NULL. Absence has one spelling, and a value that survives
     this macro is never one a further pass would reject.

  The vocabulary is not restated here: `null_semantics` renders `NULL_SENTINELS`,
  which S4.2 declares exactly once.
#}
{% macro name_norm(col) -%}
{%- set lowered = lowercase_trim(col) | trim -%}
{%- set folded = "nfc_normalize(strip_accents(" ~ lowered ~ "))" -%}
{%- set separators = "regexp_replace(" ~ folded ~ ", '[-_]', ' ', 'g')" -%}
{#- POSIX classes and no backslash, so nothing has to survive both Jinja's string
    escaping and DuckDB's literal parsing; `''` is one apostrophe to DuckDB. -#}
{%- set punctuated = "regexp_replace(" ~ separators ~ ", '[.''’,]', '', 'g')" -%}
{%- set collapsed = "trim(regexp_replace(" ~ punctuated ~ ", '[[:space:]]+', ' ', 'g'))" -%}
{{ null_semantics("nullif(" ~ collapsed ~ ", '')") | trim }}
{%- endmacro %}
