SELECT revision, media
FROM yoink_runtime.media_cache
WHERE actor_id = $1 AND bot_id = $2 AND cache_key = $3
  AND expires_at > clock_timestamp()
  AND octet_length(media::text) <= 512000;
