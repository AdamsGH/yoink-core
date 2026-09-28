UPDATE runtime_media_cache SET media = json('[{"kind":"video","file_id":"bad id","size_bytes":1}]')
WHERE actor_id = ?1 AND bot_id = ?2 AND cache_key = ?3;
