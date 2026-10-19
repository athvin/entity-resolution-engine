UPDATE public.orgs SET state = $new_state, updated_at = now() WHERE name = $name AND state = $expected
