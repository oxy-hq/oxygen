CREATE OR REPLACE FUNCTION oxy_changes.guard_ddl() RETURNS event_trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
  v_table text;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM oxy_changes.tables) THEN
    RETURN;
  END IF;
  SELECT format('%I.%I', t.schema_name, t.table_name) INTO v_table
    FROM oxy_changes.tables t
    JOIN pg_catalog.pg_class c ON c.oid = t.relid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
   WHERE t.installed
     AND (c.relname <> t.table_name
          OR n.nspname <> t.schema_name
          OR (SELECT count(*)
                FROM (VALUES ('i', 4, NULL, 'oxy_new'), ('u', 16, 'oxy_old', 'oxy_new'),
                             ('d', 8, 'oxy_old', NULL), ('t', 32, NULL, NULL))
                     AS w (op, tgtype, old_table, new_table)
               WHERE EXISTS (
                 SELECT 1
                   FROM pg_catalog.pg_trigger g
                   JOIN pg_catalog.pg_proc p ON p.oid = g.tgfoid
                   JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace
                  WHERE g.tgrelid = t.relid
                    AND g.tgenabled = 'A'
                    AND g.tgtype = w.tgtype
                    AND g.tgqual IS NULL
                    AND g.tgoldtable::text IS NOT DISTINCT FROM w.old_table
                    AND g.tgnewtable::text IS NOT DISTINCT FROM w.new_table
                    AND pn.nspname = t.schema_name
                    AND p.proname = '_oxy_capture_' || t.relid::text || '_' || w.op)) < 4)
   ORDER BY 1
   LIMIT 1;
  IF v_table IS NOT NULL THEN
    RAISE EXCEPTION 'oxy_changes: % is captured, so its four capture triggers, its name and its schema must stay as installed; remove it from oltpChanges and publish first', v_table
      USING ERRCODE = 'insufficient_privilege';
  END IF;
  SELECT format('%s calls %I.%I', g.tgrelid::regclass, pn.nspname, p.proname) INTO v_table
    FROM pg_catalog.pg_trigger g
    JOIN pg_catalog.pg_proc p ON p.oid = g.tgfoid
    JOIN pg_catalog.pg_namespace pn ON pn.oid = p.pronamespace
   WHERE pn.nspname LIKE 'app\_%'
     AND p.proname ~ '^_oxy_capture_[0-9]+_[iudt]$'
     AND substring(p.proname FROM '^_oxy_capture_([0-9]+)_[iudt]$') <> g.tgrelid::text
   ORDER BY 1
   LIMIT 1;
  IF v_table IS NOT NULL THEN
    RAISE EXCEPTION 'oxy_changes: a trigger on % generated for another table; a capture function records only the table it was generated for, so drop that trigger', v_table
      USING ERRCODE = 'insufficient_privilege';
  END IF;
END
$$