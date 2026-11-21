DELETE FROM lake.main.record_verifications
WHERE record_key = $record_key AND attribute IS NOT DISTINCT FROM $attribute
