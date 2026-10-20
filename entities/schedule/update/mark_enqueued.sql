UPDATE public.schedules SET last_enqueued_at = now(), updated_at = now()
WHERE id = $schedule_id
