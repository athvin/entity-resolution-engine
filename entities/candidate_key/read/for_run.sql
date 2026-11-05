SELECT key_columns, row_count, covered_rows, distinct_values,
       expected_pairs, largest_block_pairs, largest_block_rows,
       entropy_bits, pair_loss_from_nulls
FROM lake.main.candidate_keys AT (VERSION => $snapshot)
WHERE profile_run_id = $profile_run_id
ORDER BY expected_pairs, key_columns
