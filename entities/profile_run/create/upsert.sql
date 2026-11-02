MERGE INTO lake.main.profile_runs AS target USING __profile_run_src AS source
  ON target.subject = source.subject
 AND target.source_system = source.source_system
 AND target.corpus_digest = source.corpus_digest
 AND target.tier = source.tier
WHEN MATCHED THEN UPDATE SET
    snapshot = source.snapshot,
    ingest_batch_id = source.ingest_batch_id,
    row_count = source.row_count,
    column_count = source.column_count,
    config_hash = source.config_hash,
    model_version = source.model_version,
    sampled = source.sampled,
    sample_proportion = source.sample_proportion,
    sample_seed = source.sample_seed,
    duration_ms = source.duration_ms,
    captured_at = source.captured_at,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, subject, source_system, snapshot, tier, ingest_batch_id, corpus_digest,
     row_count, column_count, config_hash, model_version, sampled,
     sample_proportion, sample_seed, duration_ms, captured_at,
     created_at, updated_at)
    VALUES
    (sha256('profile_runs:' || source.subject || ':' || source.source_system
            || ':' || source.corpus_digest
            || ':' || cast(source.tier AS VARCHAR)),
     source.subject, source.source_system, source.snapshot, source.tier,
     source.ingest_batch_id, source.corpus_digest, source.row_count,
     source.column_count, source.config_hash, source.model_version,
     source.sampled, source.sample_proportion, source.sample_seed,
     source.duration_ms, source.captured_at,
     source.captured_at, source.captured_at)
