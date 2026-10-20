SELECT id, config_hash, model_version, std_version, survivorship_version, lexicon_hash
FROM lake.main.runs
WHERE status = 'succeeded'
ORDER BY started_at DESC, id DESC
LIMIT 1
