SELECT count(*) AS n
FROM lake.main.review_queue AT (VERSION => $snapshot)
WHERE status = 'open'
