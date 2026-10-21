DELETE FROM lake.main.scored_records
WHERE record_key NOT IN (SELECT record_key FROM __scored_src)
