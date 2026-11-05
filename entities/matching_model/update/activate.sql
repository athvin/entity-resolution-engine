UPDATE public.matching_models
SET state = CASE WHEN id = $model_id THEN 'active' ELSE 'archived' END,
    activated_at = CASE WHEN id = $model_id THEN now() ELSE activated_at END,
    updated_at = now()
WHERE dataset_id = $dataset_id
  AND (
    id = $model_id
    OR (
      state = 'active'
      AND name = (
        SELECT name FROM public.matching_models
        WHERE id = $model_id AND dataset_id = $dataset_id
      )
    )
  )
