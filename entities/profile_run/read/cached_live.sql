SELECT id, snapshot, row_count, column_count, sampled, duration_ms, captured_at
FROM lake.main.profile_runs
WHERE subject = $subject
  AND source_system = $source_system
  AND corpus_digest = $corpus_digest
  AND tier = $tier
