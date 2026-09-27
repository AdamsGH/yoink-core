SELECT to_regclass('yoink_runtime._sqlx_migrations') IS NOT NULL AS has_migrations,
       to_regclass('yoink_runtime.operations') IS NOT NULL AS has_operations;
