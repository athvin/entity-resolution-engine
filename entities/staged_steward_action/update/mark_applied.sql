UPDATE public.staged_steward_actions SET state = 'applied', applied_at = now()
WHERE action_id = $action_id
