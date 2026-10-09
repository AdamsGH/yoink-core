SELECT date(created_at/1000000,'unixepoch') AS date,count(*) AS count FROM music_resolve_log WHERE user_id=?1 AND created_at>=?2 GROUP BY date ORDER BY date;
