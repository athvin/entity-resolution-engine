SELECT rec_a_key, rec_b_key, match_probability
FROM lake.main.match_scores AT (VERSION => $snapshot)
WHERE is_active
  AND rec_a_key IN (SELECT unnest(string_split($record_keys, chr(31))))
  AND rec_b_key IN (SELECT unnest(string_split($record_keys, chr(31))))
ORDER BY rec_a_key, rec_b_key
