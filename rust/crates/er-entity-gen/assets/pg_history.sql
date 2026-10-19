-- The generic history trigger: one plpgsql function serves every
-- `history: true` entity. The companion table name arrives as TG_ARGV[0], the
-- row images are whole-row jsonb (schema-evolution-proof: no entity column is
-- named here, so ADD/DROP/ALTER COLUMN never breaks the trigger and old rows
-- keep the values of dropped columns), and the acting principal comes from the
-- er.actor GUC with a session_user fallback. Deliberately NOT
-- exception-swallowed: if history cannot be written, the write fails.
CREATE OR REPLACE FUNCTION public.er_history() RETURNS trigger
LANGUAGE plpgsql AS $fn$
DECLARE
  target text := TG_ARGV[0];
  acting text := COALESCE(NULLIF(current_setting('er.actor', true), ''), session_user);
BEGIN
  IF TG_OP = 'INSERT' THEN
    EXECUTE format(
      'INSERT INTO public.%I (row_id, op, actor, old_row, new_row) VALUES ($1, $2, $3, $4, $5)',
      target)
      USING NEW.id, 'insert', acting, NULL::jsonb, to_jsonb(NEW);
    RETURN NEW;
  ELSIF TG_OP = 'UPDATE' THEN
    EXECUTE format(
      'INSERT INTO public.%I (row_id, op, actor, old_row, new_row) VALUES ($1, $2, $3, $4, $5)',
      target)
      USING NEW.id, 'update', acting, to_jsonb(OLD), to_jsonb(NEW);
    RETURN NEW;
  ELSE
    EXECUTE format(
      'INSERT INTO public.%I (row_id, op, actor, old_row, new_row) VALUES ($1, $2, $3, $4, $5)',
      target)
      USING OLD.id, 'delete', acting, to_jsonb(OLD), NULL::jsonb;
    RETURN OLD;
  END IF;
END
$fn$;
