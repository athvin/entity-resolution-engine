{{ config(tags=['keys']) }}

-- S5.0 / D9: every pair row in `match_scores`, `assertions`, `review_queue`
-- (`subject_type='pair'`) and `cut_edges` MUST satisfy `rec_a_key < rec_b_key`
-- lexically. Canonicalisation happens in one helper at write time
-- (`er.entities.ids.canonicalize_pair`); this is the assertion that it happened.
--
-- The invariant is not tidiness. It is the precondition that makes assertion
-- application correct -- a reader of these four never performs a two-sided join,
-- so an uncanonicalised row is simply invisible to every lookup -- and it is part
-- of the reconciliation determinism total order (S4.5.4, M1).
--
-- One unioned test rather than four, and `relation` is selected so a failure names
-- which of the four broke: dbt prints the failing rows, and "a pair is unordered"
-- without the relation would send a reader to all four write paths.

with pairs as (

    select 'match_scores' as relation, rec_a_key, rec_b_key
    from {{ source('lake', 'match_scores') }}

    union all

    select 'assertions' as relation, rec_a_key, rec_b_key
    from {{ source('lake', 'assertions') }}

    union all

    -- `review_queue` also holds entity subjects, whose pair keys are NULL by
    -- design (S5); only its `subject_type='pair'` rows carry the invariant.
    select 'review_queue' as relation, rec_a_key, rec_b_key
    from {{ source('lake', 'review_queue') }}
    where subject_type = 'pair'

    union all

    select 'cut_edges' as relation, rec_a_key, rec_b_key
    from {{ source('lake', 'cut_edges') }}

)

-- The two NULL arms are load-bearing for `review_queue` alone: `rec_a_key >=
-- rec_b_key` evaluates to NULL rather than true when either side is NULL, so a
-- pair row missing a key would otherwise pass a test whose claim it does not
-- satisfy. On the other three relations both columns are NOT NULL and the arms
-- are unreachable.
select relation, rec_a_key, rec_b_key
from pairs
where rec_a_key is null
   or rec_b_key is null
   or rec_a_key >= rec_b_key
