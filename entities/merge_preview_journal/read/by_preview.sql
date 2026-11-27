SELECT id, org_id, dataset_id, merge_preview_id, from_status, to_status, fingerprint, actor,
       created_at, updated_at
FROM public.merge_preview_journal
WHERE org_id = $org_id AND dataset_id = $dataset_id AND merge_preview_id = $merge_preview_id
ORDER BY id
