SELECT json_build_object(
  'revision', (SELECT version_num FROM public.alembic_version),
  'tables', (SELECT json_agg(t ORDER BY t.name) FROM (
    SELECT c.relname AS name,
      (SELECT json_agg(a ORDER BY a.ordinal) FROM (
        SELECT at.attnum AS ordinal, at.attname AS name,
          format_type(at.atttypid, at.atttypmod) AS type,
          ty.typname AS udt, ty.typtype AS kind,
          at.attnotnull AS not_null,
          pg_get_expr(d.adbin, d.adrelid) AS default_value,
          at.attgenerated AS generated,
          pg_get_serial_sequence(format('%I.%I', n.nspname, c.relname), at.attname) AS sequence
        FROM pg_attribute at JOIN pg_type ty ON ty.oid = at.atttypid
        LEFT JOIN pg_attrdef d ON d.adrelid = at.attrelid AND d.adnum = at.attnum
        WHERE at.attrelid = c.oid AND at.attnum > 0 AND NOT at.attisdropped
      ) a) AS columns,
      (SELECT json_agg(json_build_object('name', co.conname, 'kind', co.contype, 'definition', pg_get_constraintdef(co.oid)) ORDER BY co.conname)
       FROM pg_constraint co WHERE co.conrelid = c.oid) AS constraints,
      (SELECT json_agg(pg_get_indexdef(i.indexrelid) ORDER BY i.indexrelid)
       FROM pg_index i WHERE i.indrelid = c.oid) AS indexes
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public' AND c.relkind = 'r'
  ) t),
  'views', (SELECT coalesce(json_agg(json_build_object('name', viewname, 'definition', definition) ORDER BY viewname), '[]'::json)
    FROM pg_views WHERE schemaname = 'public'),
  'triggers', (SELECT coalesce(json_agg(json_build_object('table', c.relname, 'name', t.tgname,
    'definition', pg_get_triggerdef(t.oid, true), 'function', pg_get_functiondef(t.tgfoid)) ORDER BY c.relname, t.tgname), '[]'::json)
    FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public' AND NOT t.tgisinternal),
  'enums', (SELECT json_object_agg(e.name, e.labels) FROM (
    SELECT ty.typname AS name, json_agg(en.enumlabel ORDER BY en.enumsortorder) AS labels
    FROM pg_type ty JOIN pg_enum en ON ty.oid = en.enumtypid
    JOIN pg_namespace n ON n.oid = ty.typnamespace
    WHERE n.nspname = 'public' GROUP BY ty.typname
  ) e)
);
