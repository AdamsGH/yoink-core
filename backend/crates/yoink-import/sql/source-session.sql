SELECT pg_catalog.set_config('lock_timeout', $1, true),
       pg_catalog.set_config('statement_timeout', $2, true),
       pg_catalog.set_config('timezone', 'UTC', true),
       pg_catalog.set_config('datestyle', 'ISO, YMD', true),
       pg_catalog.set_config('extra_float_digits', '3', true),
       pg_catalog.set_config('search_path', 'pg_catalog, public', true),
       pg_catalog.set_config('row_security', 'off', true);
