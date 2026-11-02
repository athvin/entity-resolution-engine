INSERT INTO public.fixups (id, name, applied_by)
VALUES ($id, $name, $applied_by)
ON CONFLICT (name) DO NOTHING
