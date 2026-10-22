MERGE INTO lake.main.record_qualities AS target USING __quality_src AS source
  ON target.record_key = source.record_key
WHEN MATCHED THEN UPDATE SET
    score = source.score,
    completeness = source.completeness,
    validity = source.validity,
    freshness = source.freshness,
    run_id = source.run_id,
    scored_at = source.scored_at,
    updated_at = source.scored_at
WHEN NOT MATCHED THEN INSERT
    (id, record_key, score, completeness, validity, freshness,
     run_id, scored_at, created_at, updated_at)
    VALUES
    (sha256('record_qualities:' || source.record_key),
     source.record_key, source.score, source.completeness, source.validity,
     source.freshness, source.run_id, source.scored_at,
     source.scored_at, source.scored_at)
