UPDATE lake.main.assertions SET active = false, retracted_at = $retracted_at::timestamp,
    updated_at = $retracted_at::timestamp
WHERE assertion_id = $assertion_id
