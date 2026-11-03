SELECT column_name, row_count, total_row_count, non_null_count, blank_count,
       distinct_approx, distinct_exact, distinct_normalized,
       len_min, len_max, len_avg, len_p50, len_p95, lex_min, lex_max,
       num_min, num_max, num_avg, num_std,
       cast_bigint, cast_double, cast_date, cast_timestamp, cast_boolean,
       entropy_bits, top_values
FROM lake.main.column_profiles AT (VERSION => $snapshot)
WHERE profile_run_id = $profile_run_id
ORDER BY column_name
