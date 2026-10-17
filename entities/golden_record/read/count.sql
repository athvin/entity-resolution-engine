SELECT count(*) AS n
FROM lake.main.golden_records AT (VERSION => $snapshot)
