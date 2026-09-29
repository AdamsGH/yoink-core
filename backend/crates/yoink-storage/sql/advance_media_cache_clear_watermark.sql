INSERT INTO runtime_media_cache_clear_watermarks(actor_id,bot_id,message_id)
VALUES(?1,?2,?3)
ON CONFLICT(actor_id,bot_id) DO UPDATE SET message_id=excluded.message_id
WHERE excluded.message_id>runtime_media_cache_clear_watermarks.message_id
RETURNING message_id;
