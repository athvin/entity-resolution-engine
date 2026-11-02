SELECT
  id,
  name,
  applied_by,
  created_at,
  updated_at
FROM public.fixups
WHERE name = $name
