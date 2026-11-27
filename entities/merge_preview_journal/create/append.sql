INSERT INTO public.merge_preview_journal
  (id, org_id, dataset_id, merge_preview_id, from_status, to_status, fingerprint, actor)
VALUES
  ($id, $org_id, $dataset_id, $merge_preview_id, $from_status, $to_status, $fingerprint, $actor)
