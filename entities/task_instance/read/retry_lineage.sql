WITH RECURSIVE lineage AS (
  SELECT t.id, t.job_id, t.task_id, t.try_number
  FROM public.task_instances t
  WHERE t.id = $task_instance_id
  UNION ALL
  SELECT p.id, p.job_id, p.task_id, p.try_number
  FROM public.task_instances p
  JOIN lineage l ON p.job_id = l.job_id AND p.task_id = l.task_id AND p.try_number = l.try_number - 1
  WHERE p.live IS NULL AND p.archived_reason = 'retry'
)
SELECT
  ti.id,
  ti.org_id,
  ti.job_id,
  ti.task_id,
  ti.seq,
  ti.try_number,
  ti.live,
  ti.archived_reason,
  ti.kind,
  ti.state,
  ti.trigger_rule,
  ti.upstream,
  ti.teardown,
  ti.segment,
  ti.resource_class,
  ti.next_resource_class,
  ti.escalated_from,
  ti.max_tries,
  ti.launches,
  ti.max_launches,
  ti.not_before,
  ti.input,
  ti.output,
  ti.outcome,
  ti.exit_code,
  ti.termination_reason,
  ti.error_class,
  ti.error_detail,
  ti.owner_epoch,
  ti.k8s_job_name,
  ti.pod_name,
  ti.pod_uid,
  ti.node_name,
  ti.image_digest,
  ti.snapshot_start,
  ti.snapshot_end,
  ti.counters,
  ti.trace_context,
  ti.token_hash,
  ti.scheduled_at,
  ti.queued_at,
  ti.started_at,
  ti.heartbeat_at,
  ti.finished_at,
  ti.created_at,
  ti.updated_at
FROM public.task_instances ti
JOIN lineage l ON l.id = ti.id
ORDER BY ti.try_number DESC
