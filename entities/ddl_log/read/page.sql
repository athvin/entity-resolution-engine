SELECT
  id,
  created_at,
  updated_at,
  actor,
  phase,
  ddl_tag,
  object_type,
  object_identity,
  command_text
FROM public.ddl_log
WHERE ($ddl_tag::text IS NULL OR ddl_tag = $ddl_tag)
  AND ($object_identity::text IS NULL OR object_identity = $object_identity)
  AND ($before_id::bigint IS NULL OR id < $before_id)
ORDER BY id DESC
LIMIT $limit
