UPDATE yoink_runtime.media_cache SET media = '[{"kind":"video","file_id":"bad id","size_bytes":1}]'::jsonb
WHERE actor_id = $1 AND bot_id = $2 AND cache_key = $3;