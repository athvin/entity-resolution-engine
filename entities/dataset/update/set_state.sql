UPDATE public.datasets SET state = $new_state, updated_at = now()
WHERE org_id = $org_id AND name = $name AND state = $expected
