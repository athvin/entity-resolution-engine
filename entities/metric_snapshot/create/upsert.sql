MERGE INTO lake.main.metric_snapshots AS target USING __metric_src AS source
  ON target.run_id = source.run_id
WHEN MATCHED THEN UPDATE SET
    captured_at = source.captured_at,
    records = source.records,
    entities = source.entities,
    duplicate_records = source.duplicate_records,
    duplicate_groups = source.duplicate_groups,
    largest_group = source.largest_group,
    review_backlog = source.review_backlog,
    auto_merge_pairs = source.auto_merge_pairs,
    review_band_pairs = source.review_band_pairs,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, run_id, captured_at, records, entities, duplicate_records, duplicate_groups,
     largest_group, review_backlog, auto_merge_pairs, review_band_pairs,
     created_at, updated_at)
    VALUES
    (sha256('metric_snapshots:' || source.run_id),
     source.run_id, source.captured_at, source.records, source.entities,
     source.duplicate_records, source.duplicate_groups, source.largest_group,
     source.review_backlog, source.auto_merge_pairs, source.review_band_pairs,
     source.captured_at, source.captured_at)
