UPDATE public.staged_steward_actions SET state = 'applied', applied_at = now(),
    updated_at = now()
WHERE id = $action_id
