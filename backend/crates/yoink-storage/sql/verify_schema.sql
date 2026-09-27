SELECT to_regclass('yoink_runtime._sqlx_migrations') IS NOT NULL AS has_migrations,
       to_regclass('yoink_runtime.operations') IS NOT NULL AS has_operations,
       EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'operations' AND column_name = 'progress_percent') AS has_bot_progress,
       to_regclass('yoink_runtime.delivery_units') IS NOT NULL AS has_delivery_units,
       EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'operations' AND column_name = 'planned_artifact_count') AS has_delivery_batch_state,
       to_regclass('yoink_runtime.media_cache') IS NOT NULL
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'operations' AND column_name = 'cache_bypassed')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'operations' AND column_name = 'cache_key')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'cache_key')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'actor_id')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'bot_id')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'revision')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'expires_at')
       AND EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'yoink_runtime' AND table_name = 'media_cache' AND column_name = 'media') AS has_media_cache;