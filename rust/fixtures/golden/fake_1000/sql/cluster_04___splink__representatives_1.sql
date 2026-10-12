-- templated: __splink__representatives_1
-- physical: __splink__representatives_1_225b6c241
CREATE TABLE __splink__representatives_1_225b6c241 AS
WITH

__splink__rep_updates as (
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

        from __splink__df_neighbours_3192c2212

        union all

        select

            representative as old_rep,
            representative,
            1 as stable

        from __splink__df_representatives_f38ff867e

    ) as rep_union
    group by old_rep
)
select

    repr.node_id,
    r.representative,
    r.stable

from __splink__rep_updates as r

left join __splink__df_representatives_f38ff867e as repr
on r.old_rep = repr.representative
