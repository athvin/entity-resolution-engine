SELECT record_key, attribute
FROM lake.main.record_verifications AT (VERSION => $snapshot)
WHERE active
