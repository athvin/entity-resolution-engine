SELECT count(*) AS n
FROM lake.main.acct_int_std_records AT (VERSION => $snapshot)
