SELECT
  webhook_id,
  org,
  url,
  events,
  secret,
  active,
  created_at
FROM public.webhooks
WHERE org = $org AND active
  AND (events @> to_jsonb($event_type::text) OR jsonb_array_length(events) = 0)
ORDER BY created_at
