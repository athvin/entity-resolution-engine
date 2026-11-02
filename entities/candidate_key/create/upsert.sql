MERGE INTO lake.main.candidate_keys AS target USING __candidate_key_src AS source
  ON target.profile_run_id = source.profile_run_id
 AND target.key_columns = source.key_columns
WHEN MATCHED THEN UPDATE SET
    row_count = source.row_count,
    covered_rows = source.covered_rows,
    distinct_values = source.distinct_values,
    expected_pairs = source.expected_pairs,
    largest_block_pairs = source.largest_block_pairs,
    largest_block_rows = source.largest_block_rows,
    entropy_bits = source.entropy_bits,
    pair_loss_from_nulls = source.pair_loss_from_nulls,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, profile_run_id, key_columns, row_count, covered_rows, distinct_values,
     expected_pairs, largest_block_pairs, largest_block_rows, entropy_bits,
     pair_loss_from_nulls, created_at, updated_at)
    VALUES
    (sha256('candidate_keys:' || source.profile_run_id || ':' || source.key_columns),
     source.profile_run_id, source.key_columns, source.row_count,
     source.covered_rows, source.distinct_values, source.expected_pairs,
     source.largest_block_pairs, source.largest_block_rows, source.entropy_bits,
     source.pair_loss_from_nulls, source.captured_at, source.captured_at)
