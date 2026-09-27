INSERT INTO yoink_runtime.media_cache (actor_id, bot_id, cache_key, revision, media, expires_at)
SELECT 79000, 899, 'v1:' || lpad(n::text, 64, '0'), $1,
       '[{"kind":"photo","file_id":"expired-id","size_bytes":1}]'::jsonb,
       clock_timestamp() - INTERVAL '1 day'
FROM generate_series(1, 40) n;
