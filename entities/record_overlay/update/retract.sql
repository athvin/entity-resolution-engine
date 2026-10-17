UPDATE lake.main.record_overlays SET active = false, retracted_at = $retracted_at::timestamp
WHERE record_key = $record_key AND kind = $kind
