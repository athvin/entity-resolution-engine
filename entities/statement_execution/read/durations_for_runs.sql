SELECT run_id, stage, statement,
       CAST(sum(duration_ms) AS BIGINT) AS total_ms,
       CAST(count(*) AS BIGINT) AS executions
FROM lake.main.statement_executions AT (VERSION => $snapshot)
WHERE run_id IN (SELECT unnest(string_split($run_ids, chr(31))))
GROUP BY run_id, stage, statement
ORDER BY run_id, stage, statement
