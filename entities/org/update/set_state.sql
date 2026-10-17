UPDATE public.orgs SET state = $new_state WHERE name = $name AND state = $expected
