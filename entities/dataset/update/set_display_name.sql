UPDATE public.datasets SET display_name = $display_name, updated_at = now()
WHERE org_id = $org_id AND name = $name
