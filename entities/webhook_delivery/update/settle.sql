UPDATE public.webhook_deliveries
SET state = CASE
              WHEN $delivered THEN 'delivered'
              WHEN $retryable AND attempt < max_attempts THEN 'pending'
              ELSE 'failed'
            END,
    -- Exponential backoff from the attempt already made, capped so a delivery
    -- cannot drift hours out: 5s, 10s, 20s, … 300s. Same curve as jobs.
    next_attempt_at = CASE
                        WHEN NOT $delivered AND $retryable AND attempt < max_attempts
                        THEN now() + make_interval(
                               secs => least(300, 5 * power(2, greatest(attempt - 1, 0)))::int)
                        ELSE next_attempt_at
                      END,
    response_code = $response_code,
    error = $error,
    delivered_at = CASE WHEN $delivered THEN now() ELSE delivered_at END,
    updated_at = now()
WHERE id = $delivery_id
RETURNING id, org_id, webhook_id, event_type, payload, state, attempt, max_attempts,
          next_attempt_at, response_code, error, delivered_at, created_at, updated_at
