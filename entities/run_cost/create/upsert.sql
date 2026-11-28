INSERT INTO public.run_costs
  (id, org_id, dataset_id, job_id, task_instance_id, task_id, resource_class, node_name,
   attempt_state, billing_period, wait_seconds, run_seconds, allocated_usd, billed_usd, billed_as_of)
VALUES
  ($id, $org_id, $dataset_id, $job_id, $task_instance_id, $task_id, $resource_class, $node_name,
   $attempt_state, $billing_period, $wait_seconds, $run_seconds, $allocated_usd, $billed_usd, $billed_as_of)
ON CONFLICT (task_instance_id) DO UPDATE
SET allocated_usd = EXCLUDED.allocated_usd, billed_usd = EXCLUDED.billed_usd,
    billed_as_of = EXCLUDED.billed_as_of, attempt_state = EXCLUDED.attempt_state, updated_at = now()
RETURNING id, org_id, dataset_id, job_id, task_instance_id, task_id, resource_class, node_name, attempt_state, billing_period, wait_seconds, run_seconds, allocated_usd, billed_usd, billed_as_of, created_at, updated_at
