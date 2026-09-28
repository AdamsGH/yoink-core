SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = 'yoink_runtime')
    OR EXISTS (
        SELECT 1 FROM pg_catalog.pg_class c
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND (c.relrowsecurity OR c.relforcerowsecurity OR c.relkind IN ('p', 'f', 'm'))
    );
