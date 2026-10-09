INSERT INTO music_resolve_log
    (user_id, source_platform, artist, title, platforms_found, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
RETURNING id
