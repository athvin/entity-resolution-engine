INSERT INTO public.compute_rates
  (id, resource_class, billing_period, usd_per_pod_hour, attempt_hours, billed_usd, instances)
VALUES ($id, $resource_class, $billing_period, $usd_per_pod_hour, $attempt_hours, $billed_usd, $instances)
ON CONFLICT (resource_class, billing_period) DO UPDATE
SET usd_per_pod_hour = EXCLUDED.usd_per_pod_hour, attempt_hours = EXCLUDED.attempt_hours,
    billed_usd = EXCLUDED.billed_usd, instances = EXCLUDED.instances, updated_at = now()
RETURNING id, resource_class, billing_period, usd_per_pod_hour, attempt_hours, billed_usd, instances, created_at, updated_at
