INSERT INTO yoink_runtime.media_cache(actor_id, bot_id, cache_key, revision, media, expires_at)
VALUES ($1, $2, $3, $4, $5, clock_timestamp() + INTERVAL '1 hour');