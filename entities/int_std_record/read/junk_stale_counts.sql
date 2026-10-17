SELECT count(*) AS n,
       count(*) FILTER (WHERE (email IS NOT NULL AND email_valid = false)
                           OR (phone_e164 IS NOT NULL AND phone_valid = false)) AS junk,
       count(*) FILTER (WHERE COALESCE(updated_at_source, ingested_at)
                              < CAST(now() AS TIMESTAMP) - $stale_days * INTERVAL '1 day') AS stale
FROM lake.main.int_std_records AT (VERSION => $snapshot)
