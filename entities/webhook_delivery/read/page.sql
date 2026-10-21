SELECT id, org_id, webhook_id, event_type, payload, state, attempt, max_attempts,
       next_attempt_at, response_code, error, delivered_at, created_at, updated_at
FROM public.webhook_deliveries
WHERE org_id = $org_id
  -- Cast explicitly: Postgres cannot infer a parameter's type from `$x IS NULL`
  -- alone, and resolves left to right, so it gives up before reaching the
  -- comparison that would have told it. (The lake's equivalent filters need no
  -- cast — DuckDB infers these; this is a Postgres-side difference, not a
  -- stylistic one.)
  AND ($webhook_id::varchar IS NULL OR webhook_id = $webhook_id::varchar)
  AND ($state::varchar IS NULL OR state = $state::varchar)
ORDER BY created_at DESC, id DESC
LIMIT $limit
