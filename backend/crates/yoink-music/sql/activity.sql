SELECT count(*) AS total,max(created_at) AS last_at FROM music_resolve_log WHERE user_id=?;
