SELECT id, run_id, captured_at, records, entities, duplicate_records, duplicate_groups,
       largest_group, review_backlog, auto_merge_pairs, review_band_pairs,
       created_at, updated_at
FROM lake.main.metric_snapshots AT (VERSION => $snapshot)
ORDER BY captured_at DESC, run_id DESC
LIMIT $limit
