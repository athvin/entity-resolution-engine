INSERT INTO lake.main.assertions
  (id, rec_a_key, rec_b_key, kind, active, created_by, created_at, note, updated_at)
VALUES ($assertion_id, $rec_a_key, $rec_b_key, $kind, true, $created_by, $created_at::timestamp, $note,
  $created_at::timestamp)
