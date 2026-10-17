INSERT INTO lake.main.assertions
  (assertion_id, rec_a_key, rec_b_key, kind, active, created_by, created_at, note)
VALUES ($assertion_id, $rec_a_key, $rec_b_key, $kind, true, $created_by, $created_at::timestamp, $note)
