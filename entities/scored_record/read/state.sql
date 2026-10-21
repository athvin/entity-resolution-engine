SELECT count(*) AS n,
       count(DISTINCT model_version || ':' || tf_snapshot_id) AS identities,
       min(model_version) AS model_version,
       min(tf_snapshot_id) AS tf_snapshot_id
FROM lake.main.scored_records
