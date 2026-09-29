DELETE FROM runtime_media_cache
WHERE actor_id=?1 AND bot_id=?2
 AND cache_key IN (
  SELECT op.cache_key FROM runtime_operations op
  WHERE op.actor_id=?1
   AND json_extract(op.request,'$.target.bot_id')=?2
   AND json_extract(op.request,'$.source_url')=?3
   AND op.cache_key IS NOT NULL
 );
