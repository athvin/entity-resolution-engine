SELECT id, snapshot_end
FROM lake.main.runs AT (VERSION => $snapshot)
WHERE status = 'succeeded' AND snapshot_end IS NOT NULL
  AND (snapshot_end > $after_snapshot OR (snapshot_end = $after_snapshot AND id > $after_run))
ORDER BY snapshot_end, id
LIMIT $limit
