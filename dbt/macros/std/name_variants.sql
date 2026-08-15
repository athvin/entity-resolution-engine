{#
  S4.2 `name_variants(col)` -- the `LIST(VARCHAR) NOT NULL` column of S5, and the
  input `S4.3.1`'s `variant_match` (`ArrayIntersectLevel`, `min_intersection=1`)
  intersects.

  **The symmetry guarantee.** S4.2 states it as a property of every record: the
  normalized `given_name` is ALWAYS element 0 of its own array. That is what makes
  `variant_match` orientation-independent, and it is why the array is built by
  PREPENDING the name to its neighbour set rather than by unioning the two and
  sorting the result -- a sort would put `bob` before `robert` in one record and
  after it in the other, and T-MATCH-SYM (S8.3) would then be an empirical
  observation about which names happen to sort where rather than a theorem.

  **NOT NULL.** S5 types the column `LIST(VARCHAR) NOT NULL`, so this expression is
  never SQL NULL: it is the EMPTY list exactly when `name_norm(col)` is NULL, and
  otherwise a list whose element 0 is exactly `name_norm(col)`.

  **The tail is sorted and de-duplicated** so that two records carrying the same
  given name produce byte-identical arrays. That is a determinism requirement, not
  a cosmetic one: `int_std_records` feeds `std_hash`, which T-STD-1 (S8.3) hashes
  over `array_to_string(name_variants, '\x1f')`.

  **One hop, never a transitive closure.** The seed is read symmetrically -- a row
  `(a, b)` contributes `b` to `a`'s variants and `a` to `b`'s, which is what the
  `union all` below expresses -- but the walk stops there. `bob -> {bob, robert}`
  and `bobby -> {bobby, robert}` already intersect on `robert`, so
  `min_intersection=1` fires without inflating the graph; a closure would instead
  make every name reachable from every nickname that shares any hub.

  The seed is reached with `ref` and not by name: `nickname_variants` is a dbt
  seed, so the reference is what puts it in the model's DAG rather than leaving it
  a table the model hopes someone loaded.
#}
{% macro name_variants(col) -%}
{%- set normalized = name_norm(col) | trim -%}
{%- set seed = ref('nickname_variants') -%}
case when {{ normalized }} is null then cast([] as varchar[])
else list_prepend({{ normalized }}, coalesce((
  select list_sort(list_distinct(list(edge.variant_other)))
  from (select variant_a as variant_self, variant_b as variant_other from {{ seed }}
        union all
        select variant_b as variant_self, variant_a as variant_other from {{ seed }}) as edge
  where edge.variant_self = {{ normalized }}
), cast([] as varchar[])))
end
{%- endmacro %}
