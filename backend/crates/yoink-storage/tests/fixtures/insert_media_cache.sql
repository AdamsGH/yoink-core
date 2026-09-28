INSERT INTO runtime_media_cache(actor_id, bot_id, cache_key, revision, media, expires_at)
VALUES (?1, ?2, ?3, ?4, ?5, CAST(unixepoch('subsec') * 1000000 AS INTEGER) + 3600000000);
