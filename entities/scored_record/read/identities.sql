SELECT model_version,
       tf_snapshot_id,
       count(*) AS n
FROM lake.main.scored_records
GROUP BY model_version, tf_snapshot_id
ORDER BY model_version, tf_snapshot_id
