SELECT artist,count(*) AS count FROM music_resolve_log WHERE user_id=? AND artist IS NOT NULL GROUP BY artist ORDER BY count DESC,artist LIMIT 10;
