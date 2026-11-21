UPDATE lake.main.record_verifications SET active = false, retracted_at = $retracted_at::timestamp,
    updated_at = $retracted_at::timestamp
WHERE record_key = $record_key AND attribute IS NOT DISTINCT FROM $attribute
