SELECT count(*) AS n
FROM lake.main.int_std_records AT (VERSION => $snapshot)
