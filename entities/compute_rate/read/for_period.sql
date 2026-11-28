SELECT id, resource_class, billing_period, usd_per_pod_hour, attempt_hours, billed_usd, instances, created_at, updated_at
FROM public.compute_rates
WHERE billing_period = $billing_period
ORDER BY resource_class
