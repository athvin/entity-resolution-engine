SELECT run_id, count(*) AS n
FROM lake.main.match_scores AT (VERSION => $snapshot)
WHERE is_active AND match_probability >= $threshold
GROUP BY run_id
