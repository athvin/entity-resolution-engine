SELECT id, org_id, dataset_id, job_id, task_instance_id, task_id, resource_class, node_name, attempt_state, billing_period, wait_seconds, run_seconds, allocated_usd, billed_usd, billed_as_of, created_at, updated_at
FROM public.run_costs
WHERE org_id = $org_id AND billing_period = $billing_period
ORDER BY id
