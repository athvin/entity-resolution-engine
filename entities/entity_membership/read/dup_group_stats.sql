SELECT count(*) AS groups,
       CAST(coalesce(sum(c), 0) AS BIGINT) AS records_in_groups
FROM (
  SELECT entity_id, count(*) AS c
  FROM lake.main.entity_membership AT (VERSION => $snapshot)
  GROUP BY entity_id
  HAVING count(*) >= 2
) g
