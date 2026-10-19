-- The DDL ledger's event triggers. Installed OUTSIDE the baseline because
-- CREATE EVENT TRIGGER needs superuser/rds_superuser: boot applies this
-- best-effort (a privilege failure is a logged warning), and production
-- installs it through the deploy migrate Job running with the master DSN.
-- The functions swallow their own errors — the ledger must never fail DDL.
CREATE OR REPLACE FUNCTION public.er_ddl_log_end() RETURNS event_trigger
LANGUAGE plpgsql AS $ddl$
DECLARE r record;
BEGIN
  IF current_setting('er.ddl_log_skip', true) = 'on' THEN RETURN; END IF;
  FOR r IN SELECT * FROM pg_event_trigger_ddl_commands() LOOP
    IF r.command_tag NOT LIKE 'DROP %' THEN
      INSERT INTO public.ddl_log (actor, phase, ddl_tag, object_type, object_identity, command_text)
      VALUES (COALESCE(NULLIF(current_setting('er.actor', true), ''), session_user),
              'ddl_command_end', r.command_tag, r.object_type, r.object_identity,
              current_query());
    END IF;
  END LOOP;
EXCEPTION WHEN OTHERS THEN NULL;
END;
$ddl$;

CREATE OR REPLACE FUNCTION public.er_ddl_log_drop() RETURNS event_trigger
LANGUAGE plpgsql AS $ddl$
DECLARE r record;
BEGIN
  IF current_setting('er.ddl_log_skip', true) = 'on' THEN RETURN; END IF;
  -- one row per DIRECTLY-dropped object; dependent objects (composite/array
  -- types, TOAST tables) would triple-log every DROP TABLE.
  FOR r IN SELECT * FROM pg_event_trigger_dropped_objects() WHERE original LOOP
    INSERT INTO public.ddl_log (actor, phase, ddl_tag, object_type, object_identity, command_text)
    VALUES (COALESCE(NULLIF(current_setting('er.actor', true), ''), session_user),
            'sql_drop', tg_tag, r.object_type, r.object_identity, current_query());
  END LOOP;
EXCEPTION WHEN OTHERS THEN NULL;
END;
$ddl$;

DO $install$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_event_trigger WHERE evtname = 'er_ddl_audit_end') THEN
    CREATE EVENT TRIGGER er_ddl_audit_end ON ddl_command_end
      EXECUTE FUNCTION public.er_ddl_log_end();
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_event_trigger WHERE evtname = 'er_ddl_audit_drop') THEN
    CREATE EVENT TRIGGER er_ddl_audit_drop ON sql_drop
      EXECUTE FUNCTION public.er_ddl_log_drop();
  END IF;
END
$install$;
