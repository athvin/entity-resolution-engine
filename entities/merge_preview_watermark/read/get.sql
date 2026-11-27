SELECT id, org_id, dataset_id, run_snapshot_end, run_id, lake_snapshot, job_id, created_at, updated_at
FROM public.merge_preview_watermarks
WHERE org_id = $org_id AND dataset_id = $dataset_id
