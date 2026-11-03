WITH latest AS (
  SELECT id, captured_at
  FROM lake.main.profile_runs AT (VERSION => $snapshot)
  WHERE subject = $subject
    AND source_system = $source_system
    AND sampled = false
  ORDER BY captured_at DESC, id DESC
  LIMIT 1
),
shapes AS (
  SELECT profile_run_id, column_name, row_count, non_null_count, blank_count
  FROM lake.main.column_profiles AT (VERSION => $snapshot)
)
SELECT shapes.column_name, shapes.row_count, shapes.non_null_count,
       shapes.blank_count, latest.captured_at
FROM shapes
JOIN latest ON latest.id = shapes.profile_run_id
ORDER BY shapes.column_name
