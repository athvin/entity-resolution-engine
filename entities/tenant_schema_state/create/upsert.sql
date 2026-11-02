INSERT INTO public.tenant_schema_states (id, org_id, dataset_id, catalog_hash, status, detail)
VALUES ($id, $org_id, $dataset_id, $catalog_hash, $status, $detail)
ON CONFLICT (dataset_id) WHERE dataset_id IS NOT NULL DO UPDATE SET
  catalog_hash = EXCLUDED.catalog_hash,
  status = EXCLUDED.status,
  detail = EXCLUDED.detail,
  checked_at = now(),
  updated_at = now()
