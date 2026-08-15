{#
  S4.2 `lowercase_trim(col)` -- NFC-normalize, trim, lower; empty string -> NULL.

  The order is the one S4.2 writes and it is load-bearing in both directions.
  NFC comes first because `content_hash` (S4.1) hashes NFC text, so a macro that
  lowered before composing could hand the matcher a spelling the hash never saw.
  `nullif(..., '')` comes last because the empty string and NULL are different
  evidence everywhere downstream: `null_semantics` maps `''` to NULL, S6.1's
  `validated` chain orders `<attr>_valid DESC NULLS LAST`, and S4.2's blocking
  contract refuses to emit a key for either -- all of which need "absent" to be
  spelled exactly one way.

  NFC also appears AFTER `lower`, and that is not redundancy. Lowering can create
  a sequence that was not composable before it: U+0059 U+030A ("Y" with a combining
  ring) has no precomposed form and survives NFC unchanged, but its lowercase
  U+0079 U+030A composes to U+1E99. A single leading NFC therefore does not make
  the OUTPUT NFC -- and the output is what `content_hash` hashes and what every
  comparison level sees, so it is the string that has to be normalized. It is also
  what makes the macro idempotent, which S8.4 requires and which a hypothesis case
  found the moment it was missing.

  DuckDB's `trim` removes spaces and not other whitespace. That is deliberately
  left as it is: S4.2 says `trim`, and widening it here would be this file
  inventing a normalization rule the spec does not state.
#}
{% macro lowercase_trim(col) -%}
nullif(nfc_normalize(lower(trim(nfc_normalize({{ col }})))), '')
{%- endmacro %}
