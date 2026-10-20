-- templated: __dedupe__representatives_3
-- physical: __dedupe__representatives_3_ce29e8634
CREATE TABLE __dedupe__representatives_3_ce29e8634 AS
WITH

__dedupe__rep_updates as (
    select

    old_rep,
    min(representative) as representative,
    min(stable) as stable

    from
    (

        select

            node_rep as old_rep,
            neighbour_rep as representative,
            0 as stable

        from __dedupe__filtered_neighbours_2_c8c92fe41

        union all

        select

            representative as old_rep,
            representative,
            1 as stable

        from __dedupe__representatives_unstable_2_e52f93c9f

    ) as rep_union
    group by old_rep
)
select

    repr.node_id,
    r.representative,
    r.stable

from __dedupe__rep_updates as r

left join __dedupe__representatives_unstable_2_e52f93c9f as repr
on r.old_rep = repr.representative
