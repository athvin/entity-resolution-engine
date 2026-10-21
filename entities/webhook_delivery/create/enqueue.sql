INSERT INTO public.webhook_deliveries
  (id, org_id, webhook_id, event_type, payload, state, attempt, max_attempts)
VALUES ($id, $org_id, $webhook_id, $event_type, $payload, 'pending', 0, $max_attempts)
RETURNING id, org_id, webhook_id, event_type, payload, state, attempt, max_attempts,
          next_attempt_at, response_code, error, delivered_at, created_at, updated_at
