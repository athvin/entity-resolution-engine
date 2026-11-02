MERGE INTO lake.main.column_profiles AS target USING __column_profile_src AS source
  ON target.profile_run_id = source.profile_run_id
 AND target.column_name = source.column_name
WHEN MATCHED THEN UPDATE SET
    row_count = source.row_count,
    total_row_count = source.total_row_count,
    non_null_count = source.non_null_count,
    blank_count = source.blank_count,
    distinct_approx = source.distinct_approx,
    distinct_exact = source.distinct_exact,
    distinct_normalized = source.distinct_normalized,
    len_min = source.len_min,
    len_max = source.len_max,
    len_avg = source.len_avg,
    len_p50 = source.len_p50,
    len_p95 = source.len_p95,
    lex_min = source.lex_min,
    lex_max = source.lex_max,
    num_min = source.num_min,
    num_max = source.num_max,
    num_avg = source.num_avg,
    num_std = source.num_std,
    cast_bigint = source.cast_bigint,
    cast_double = source.cast_double,
    cast_date = source.cast_date,
    cast_timestamp = source.cast_timestamp,
    cast_boolean = source.cast_boolean,
    entropy_bits = source.entropy_bits,
    top_values = source.top_values,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, profile_run_id, column_name, row_count, total_row_count,
     non_null_count, blank_count, distinct_approx, distinct_exact,
     distinct_normalized, len_min, len_max, len_avg, len_p50, len_p95,
     lex_min, lex_max, num_min, num_max, num_avg, num_std,
     cast_bigint, cast_double, cast_date, cast_timestamp, cast_boolean,
     entropy_bits, top_values, created_at, updated_at)
    VALUES
    (sha256('column_profiles:' || source.profile_run_id || ':' || source.column_name),
     source.profile_run_id, source.column_name, source.row_count,
     source.total_row_count, source.non_null_count, source.blank_count,
     source.distinct_approx, source.distinct_exact, source.distinct_normalized,
     source.len_min, source.len_max, source.len_avg, source.len_p50,
     source.len_p95, source.lex_min, source.lex_max, source.num_min,
     source.num_max, source.num_avg, source.num_std, source.cast_bigint,
     source.cast_double, source.cast_date, source.cast_timestamp,
     source.cast_boolean, source.entropy_bits, source.top_values,
     source.captured_at, source.captured_at)
