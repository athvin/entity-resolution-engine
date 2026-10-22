SELECT event_type, count(*) AS n
FROM lake.main.entity_events
WHERE run_id = $run_id
GROUP BY event_type
ORDER BY event_type
