DELETE FROM lake.main.record_qualities
WHERE record_key NOT IN (SELECT record_key FROM __quality_src)
