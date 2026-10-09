SELECT id, user_id, source_platform, artist, title, platforms_found, created_at
FROM music_resolve_log
WHERE user_id = ?1 AND (?2 IS NULL OR id > ?2)
ORDER BY id
LIMIT ?3
