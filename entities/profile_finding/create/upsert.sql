MERGE INTO lake.main.profile_findings AS target USING __profile_finding_src AS source
  ON target.profile_run_id = source.profile_run_id
 AND target.code = source.code
 AND target.column_name = source.column_name
WHEN MATCHED THEN UPDATE SET
    severity = source.severity,
    message = source.message,
    detail = source.detail,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, profile_run_id, code, column_name, severity, message, detail,
     created_at, updated_at)
    VALUES
    (sha256('profile_findings:' || source.profile_run_id || ':' || source.code
            || ':' || source.column_name),
     source.profile_run_id, source.code, source.column_name, source.severity,
     source.message, source.detail, source.captured_at, source.captured_at)
