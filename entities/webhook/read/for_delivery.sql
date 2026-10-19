SELECT
  id,
  org_id,
  url,
  events,
  secret,
  active,
  created_at,
  updated_at
FROM public.webhooks
WHERE org_id = $org_id AND active
  AND (events @> to_jsonb($event_type::text) OR jsonb_array_length(events) = 0)
ORDER BY created_at
