DELETE FROM public.webhook_deliveries
WHERE id IN (
  SELECT id FROM public.webhook_deliveries
  WHERE state <> 'pending'
    AND created_at < now() - make_interval(days => $retention_days::bigint::int)
  LIMIT $batch
)
