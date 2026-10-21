UPDATE public.webhook_deliveries
SET attempt = webhook_deliveries.attempt + 1,
    updated_at = now()
WHERE id = (
  SELECT d.id FROM public.webhook_deliveries d
  WHERE d.state = 'pending'
    AND (d.next_attempt_at IS NULL OR d.next_attempt_at <= now())
  ORDER BY d.created_at, d.id
  FOR UPDATE SKIP LOCKED
  LIMIT 1)
RETURNING id, org_id, webhook_id, event_type, payload, state, attempt, max_attempts,
          next_attempt_at, response_code, error, delivered_at, created_at, updated_at
