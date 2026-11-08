DELETE FROM public.audit_log
WHERE id IN (
  SELECT id FROM public.audit_log
  WHERE at < now() - make_interval(days => $retention_days::bigint::int)
  LIMIT $batch
)
